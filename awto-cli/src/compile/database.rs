use std::fmt::Write;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use clap::Parser;
use heck::SnakeCase;
use log::info;
use proc_macro2::TokenTree;
use tokio::fs;

use crate::{
    compile::build_awto_pkg,
    util::{add_package_to_workspace, CargoFile},
    Runnable,
};

use super::prepare_awto_dir;

/// Compiles database package from app schema
#[derive(Parser)]
pub struct Database {
    /// Prints more information
    #[clap(short, long)]
    pub verbose: bool,
}

#[async_trait]
impl Runnable for Database {
    async fn run(&mut self) -> Result<()> {
        let cargo_file = CargoFile::load("./schema/Cargo.toml")
            .await
            .context("could not load schema Cargo.toml file from './schema/Cargo.toml'")?;
        if cargo_file
            .package
            .as_ref()
            .map(|package| package.name != "schema")
            .unwrap_or(false)
        {
            match cargo_file.package {
                Some(package) => {
                    return Err(anyhow!(
                        "schema package must be named 'schema' but is named '{}'",
                        package.name
                    ));
                }
                None => return Err(anyhow!("schema package must be named 'schema'")),
            }
        }

        prepare_awto_dir().await?;

        Self::prepare_database_dir().await?;
        add_package_to_workspace("awto/database").await?;
        build_awto_pkg("database").await?;

        info!("compiled package 'database'");

        Ok(())
    }

    fn is_verbose(&self) -> bool {
        self.verbose
    }
}

impl Database {
    const DATABASE_DIR: &'static str = "./awto/database";
    const DATABASE_SRC_DIR: &'static str = "./awto/database/src";
    const DATABASE_CARGO_PATH: &'static str = "./awto/database/Cargo.toml";
    const DATABASE_CARGO_TOML_BYTES: &'static [u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/templates/database/Cargo.toml"
    ));
    const DATABASE_BUILD_PATH: &'static str = "./awto/database/build.rs";
    const DATABASE_BUILD_BYTES: &'static [u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/templates/database/build.rs"
    ));
    const DATABASE_LIB_PATH: &'static str = "./awto/database/src/lib.rs";

    async fn prepare_database_dir() -> Result<()> {
        if Path::new(Self::DATABASE_DIR).is_dir() {
            fs::remove_dir_all(Self::DATABASE_DIR)
                .await
                .with_context(|| format!("could not delete directory '{}'", Self::DATABASE_DIR))?;
        }

        fs::create_dir(Self::DATABASE_DIR)
            .await
            .with_context(|| format!("could not create directory '{}'", Self::DATABASE_DIR))?;

        fs::create_dir(Self::DATABASE_SRC_DIR)
            .await
            .with_context(|| format!("could not create directory '{}'", Self::DATABASE_SRC_DIR))?;

        fs::write(Self::DATABASE_CARGO_PATH, Self::DATABASE_CARGO_TOML_BYTES)
            .await
            .with_context(|| format!("could not write file '{}'", Self::DATABASE_CARGO_PATH))?;

        fs::write(Self::DATABASE_BUILD_PATH, Self::DATABASE_BUILD_BYTES)
            .await
            .with_context(|| format!("could not write file '{}'", Self::DATABASE_BUILD_PATH))?;

        let schema_models = Self::read_schema_models().await?;
        let mut lib_content = concat!(
            "// This file is automatically @generated by ",
            env!("CARGO_PKG_NAME"),
            " v",
            env!("CARGO_PKG_VERSION"),
            "\n\npub use sea_orm;\n\ninclude!(concat!(env!(\"OUT_DIR\"), \"/app.rs\"));\n"
        )
        .to_string();
        for model in schema_models {
            let model_name = model.to_snake_case();
            writeln!(lib_content, "\n/// {} database model", model).unwrap();
            writeln!(lib_content, "pub mod {} {{", model_name).unwrap();
            writeln!(
                lib_content,
                r#"    sea_orm::include_model!("{}");"#,
                model_name
            )
            .unwrap();
            writeln!(lib_content, r#"}}"#).unwrap();
        }

        fs::write(Self::DATABASE_LIB_PATH, lib_content)
            .await
            .with_context(|| format!("could not write file '{}'", Self::DATABASE_LIB_PATH))?;

        Ok(())
    }

    async fn read_schema_models() -> Result<Vec<String>> {
        let schema_lib = fs::read_to_string("./schema/src/lib.rs")
            .await
            .context("could not read file './schema/src/lib.rs'")?;
        let lib = syn::parse_file(&schema_lib).context("could not parse schema source code")?;
        lib.items
            .into_iter()
            .find_map(|item| {
                if let syn::Item::Macro(syn::ItemMacro { mac, .. }) = item {
                    let macro_name = mac
                        .path
                        .segments
                        .iter()
                        .map(|segment| segment.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::");
                    if macro_name != "awto::register_schemas" && macro_name != "register_schemas" {
                        return None;
                    }

                    let models: Vec<_> = mac
                        .tokens
                        .into_iter()
                        .filter_map(|token| match token {
                            TokenTree::Ident(ident) => Some(ident.to_string()),
                            _ => None,
                        })
                        .collect();

                    Some(models)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                anyhow!("no schemas registered with the 'awto::register_schemas!' macro\n\n   Schemas must be registered:\n      `awto::register_schemas!(SchemaOne, SchemaTwo)`")
            })
    }
}
