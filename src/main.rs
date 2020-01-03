#![feature(async_closure)]
use anyhow::{self, bail, Context};
use structopt::StructOpt;
use std::path::PathBuf;
use thiserror::Error;
use surf;
use std::str::FromStr;
use serde_derive::{ Deserialize, Serialize };
use serde_yaml;

#[derive(StructOpt)]
struct Options {
    #[structopt(long, help = "Set a service to a version (e.g., --set foo=1.0.0)")]
    set: Vec<String>,
    #[structopt(short, long, help = "Output the umbrella chart to a given directory. Defaults to \"./chart\".")]
    output: Option<PathBuf>,

    #[structopt(short, long, help = "The fully-qualified URL to the helm chart repository you wish to pull services from.")]
    repository: Option<String>
}

struct ServiceSetting {
    name: String,
    version: Option<String>,
    repository: Option<String>
}

impl FromStr for ServiceSetting {
    type Err = ServiceSettingParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bits: Vec<_> = s.split("=").collect();
        if bits.len() < 2 {
            return Err(ServiceSettingParseError::ExpectedEquals(s.to_string()))
        }

        if bits[0].len() < 1 {
            return Err(ServiceSettingParseError::ExpectedServiceName(s.to_string()))

        }

        Ok(ServiceSetting {
            name: bits[0].to_string(),
            version: if bits[1].len() > 0 {
                Some(bits[1].to_string())
            } else {
                None
            },
            repository: None
        })
    }
}

#[derive(Error, Debug)]
pub enum ServiceSettingParseError {
    #[error("Expected string to contain \"=\", got {0} instead")]
    ExpectedEquals(String),
    #[error("Expected service to have a name ahead of \"=\",, got {0} instead")]
    ExpectedServiceName(String),
}

#[derive(Serialize, Deserialize, Debug)]
struct Requirement {
    name: String,
    version: String,
    repository: Option<String>
}

#[derive(Deserialize, Debug, Clone)]
#[allow(non_snake_case)]
struct ConsulValue {
    CreateIndex: u32,
    Flags: u32,
    Key: String,
    LockIndex: u32,
    ModifyIndex: u32,
    Value: String,
}

#[async_std::main]
async fn main() -> anyhow::Result<()> {
    println!("Hello, world!");
    let mut opts = Options::from_args();
    if opts.output.is_none() {
        let default = std::env::var("VONGFORM_OUTPUT_DIR").ok().unwrap_or_else(|| "chart".to_string());
        opts.output.replace(PathBuf::from(default));
    }

    if opts.repository.is_none() {
        if let Ok(repo) = std::env::var("VONGFORM_DEFAULT_REPOSITORY") {
            opts.repository.replace(repo);
        }
    }

    let settings: Vec<ServiceSetting> = opts.set.iter().filter_map(
        |xs| xs.parse().ok()
    ).collect();

    if settings.len() != opts.set.len() {
        bail!("Could not parse one of the settings (all settings require service name and an equals-sign)");
    }

    let consul_url = std::env::var("CONSUL_HTTP_ADDR").ok().unwrap_or_else(
        || "http://localhost:8500".to_string()
    );

    let mut response = surf2anyhow(surf::get(format!("{}/v1/kv/umbrella", &consul_url)).await)?;
    let body: Vec<ConsulValue> = if response.status().as_u16() == 200 {
        response.body_json().await?
    } else {
        vec![ConsulValue {
            CreateIndex: 0,
            Flags: 0,
            Key: "/umbrella".to_string(),
            LockIndex: 0,
            ModifyIndex: 0,
            Value: "W10K".to_string() // "[]", base64'd
        }]
    };

    if body.len() < 1 {
        bail!("Expected at least one item (or default), got {:?} instead", body);
    }

    let raw_yaml = base64::decode(&body[0].Value).context("Attempting to decode Consul value from Base64")?;
    let mut results: Vec<Requirement> = serde_yaml::from_slice(&raw_yaml[..]).context("Attempting to parse YAML from Consul")?;

    for setting in settings {
        let maybe_found = results.iter().enumerate().find(|(idx, xs)| xs.name == setting.name);
        match setting.version {
            Some(version) => {
                if let Some((idx, requirement)) = maybe_found {
                    results[idx].version = version;
                    results[idx].repository = opts.repository.clone().or_else(|| results[idx].repository.clone());
                } else {
                    results.push(Requirement {
                        name: setting.name,
                        version: version,
                        repository: opts.repository.clone()
                    })
                }
            },
            None => {
                if let Some((idx, requirement)) = maybe_found {
                    results.remove(idx);
                }
            }
        }
    }

    // TODO:
    // - add values overrides
    // - materialize the umbrella chart to disk

    #[derive(Serialize)]
    struct RequirementsYAML {
        dependencies: Vec<Requirement>
    }
    let compiled = RequirementsYAML { dependencies: results };
    let mut response = surf2anyhow(surf::put(
        format!("{}/v1/kv/umbrella?cas={}", &consul_url, body[0].ModifyIndex)
    ).body_string(serde_yaml::to_string(&compiled)?).await)?;

    Ok(())
}

fn surf2anyhow<T>(input: Result<T, surf::Exception>) -> anyhow::Result<T> {
    match input {
        Ok(r) => Ok(r),
        Err(e) => bail!(e)
    }
}
