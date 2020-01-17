#![feature(async_closure)]
use anyhow::{self, bail, Context};
use async_std::fs as afs;
use chrono::prelude::*;
use serde_derive::{ Deserialize, Serialize };
use serde_yaml;
use std::collections::{ HashSet, HashMap };
use std::fs::DirBuilder;
use std::path::PathBuf;
use std::str::FromStr;
use structopt::clap::AppSettings::*;
use structopt::StructOpt;
use surf;
use thiserror::Error;

#[derive(StructOpt)]
#[structopt(name = "vongform", about = "Manage data for a helm umbrella chart stored in consul. Update service versions and emit the chart.")]
#[structopt(global_setting(ColoredHelp))]
struct Options {
    #[structopt(long,
        help = "set a service to a version; can be repeated:\nvong --set sessions-2020=1.0.0 --set auth-2020=1.2.3",
    )]
    set: Vec<String>,

    #[structopt(short, long,
        help = "output the umbrella chart to the given directory;\nchecks VONGFORM_OUTPUT_DIR and falls back to `./chart'",
    )]
    output: Option<PathBuf>,

    #[structopt(short, long,
        help = "the fully-qualified url of the helm chart repository to use; defaults to VONGFORM_DEFAULT_REPOSITORY",
    )]
    repository: Option<String>
}

struct ServiceSetting {
    name: String,
    version: Option<String>
}

impl FromStr for ServiceSetting {
    type Err = ServiceSettingParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bits: Vec<_> = s.split('=').collect();
        if bits.len() < 2 {
            return Err(ServiceSettingParseError::ExpectedEquals(s.to_string()))
        }

        if bits[0].is_empty() {
            return Err(ServiceSettingParseError::ExpectedServiceName(s.to_string()))

        }

        Ok(ServiceSetting {
            name: bits[0].to_string(),
            version: if bits[1].is_empty() {
                None
            } else {
                Some(bits[1].to_string())
            }
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

#[derive(Serialize, Deserialize)]
struct RequirementsYAML {
    dependencies: Vec<Requirement>
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
            Value: "e2RlcGVuZGVuY2llczogW119Cg==".to_string() // "{dependencies: []}", base64'd
        }]
    };

    if body.is_empty() {
        bail!("Expected at least one item (or default), got {:?} instead", body);
    }

    let raw_yaml = base64::decode(&body[0].Value).context("Attempting to decode Consul value from Base64")?;
    let requirements: RequirementsYAML = serde_yaml::from_slice(&raw_yaml[..]).context("Attempting to parse YAML from Consul")?;
    let mut results = requirements.dependencies;

    for setting in settings {
        let maybe_found = results.iter().enumerate().find(|(_idx, xs)| xs.name == setting.name);
        match setting.version {
            Some(version) => {
                if let Some((idx, _requirement)) = maybe_found {
                    results[idx].version = version;
                    results[idx].repository = opts.repository.clone().or_else(|| results[idx].repository.clone());
                } else {
                    results.push(Requirement {
                        name: setting.name,
                        version,
                        repository: opts.repository.clone()
                    })
                }
            },
            None => {
                if let Some((idx, _requirement)) = maybe_found {
                    results.remove(idx);
                }
            }
        }
    }

    // TODO:
    // - materialize the umbrella chart to disk

    let mut service_names = results.iter().map(|xs| &xs.name[..]).collect::<HashSet<&str>>();
    service_names.insert("global");

    let overrides = get_overrides(service_names, &consul_url).await?;
    let now: DateTime<Utc> = Utc::now();
    let compiled = RequirementsYAML { dependencies: results };
    let requirements_yaml = serde_yaml::to_string(&compiled)?;

    let mut pb = PathBuf::from(opts.output.unwrap());
    DirBuilder::new().recursive(true).create(&pb)?;

    pb.push("Chart.yaml");
    afs::write(&pb, format!(r#"apiVersion: 'v1'
description: 'Umbrella chart, generated on {}'
appVersion: '1.0'
name: chart
version: '1.0.0-{}'
"#, now.to_rfc2822(), now.timestamp())).await?;
    pb.pop();

    pb.push("values.yaml");
    afs::write(&pb, serde_yaml::to_string(&overrides)?).await?;
    pb.pop();

    pb.push("requirements.yaml");
    afs::write(&pb, &requirements_yaml[..]).await?;
    pb.pop();

    let mut _response = surf2anyhow(surf::put(
        format!("{}/v1/kv/umbrella?cas={}", &consul_url, body[0].ModifyIndex)
    ).body_string(requirements_yaml).await)?;

    Ok(())
}

#[derive(Serialize, Debug)]
#[serde(untagged)]
enum Tree {
    Leaf(String),
    Node(HashMap<String, Tree>)
}

async fn get_overrides<'a>(service_names: HashSet<&'a str>, consul_url: &'a str) -> anyhow::Result<Tree> {
    let mut overrides = HashMap::new();

    for service_name in service_names {
        let mut response = surf2anyhow(surf::get(format!("{}/v1/kv/{}?recurse=true", &consul_url, service_name)).await)?;
        if response.status().as_u16() != 200 {
            continue;
        }

        let body: Vec<ConsulValue> = response.body_json().await?;

        for consul_value in body {
            let mut segments: Vec<_> = consul_value.Key.split('/').map(str::to_string).collect();
            let bytes = match base64::decode(&consul_value.Value) {
                Err(_) => continue,
                Ok(b) => b,
            };

            let decoded = match std::str::from_utf8(&bytes) {

                Err(_) => continue,
                Ok(b) => b.to_string(),
            };

            segments.reverse();

            let mut current = &mut overrides;

            while segments.len() > 1 {
                let level = segments.pop().unwrap();
                let tmp = current.entry(level).and_modify(|e| {
                    if let Tree::Leaf(_) = *e {
                        *e = Tree::Node(HashMap::new())
                    }
                }).or_insert(Tree::Node(HashMap::new()));

                current = match tmp {
                    Tree::Leaf(_) => {
                        unreachable!("You can't get here from there.")
                    },
                    Tree::Node(x) => x
                };
            }
            current.entry(segments.pop().unwrap())
                .and_modify(|e| *e = Tree::Leaf(String::from(&decoded[..])))
                .or_insert_with(|| Tree::Leaf(String::from(&decoded[..])));
        }
    }

    Ok(Tree::Node(overrides))
}

fn surf2anyhow<T>(input: Result<T, surf::Exception>) -> anyhow::Result<T> {
    match input {
        Ok(r) => Ok(r),
        Err(e) => bail!(e)
    }
}
