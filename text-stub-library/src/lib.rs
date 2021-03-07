// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

pub mod yaml;

use yaml::*;
use yaml_rust::ScanError;

/// Version of a TBD document.
#[derive(Copy, Clone, Debug)]
pub enum TBDVersion {
    V1,
    V2,
    V3,
    V4,
}

/// A parsed TBD record from a YAML document.
pub enum TBDRecord {
    V1(TBDVersion1),
    V2(TBDVersion2),
    V3(TBDVersion3),
    V4(TBDVersion4),
}

/// Represents an error when parsing TBD YAML.
#[derive(Debug)]
pub enum ParseError {
    YamlError(yaml_rust::ScanError),
    DocumentCountMismatch,
    Serde(serde_yaml::Error),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::YamlError(e) => e.fmt(f),
            Self::DocumentCountMismatch => {
                f.write_str("mismatch in expected document count when parsing YAML")
            }
            Self::Serde(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<yaml_rust::ScanError> for ParseError {
    fn from(e: ScanError) -> Self {
        Self::YamlError(e)
    }
}

impl From<serde_yaml::Error> for ParseError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::Serde(e)
    }
}

const TBD_V2_DOCUMENT_START: &str = "--- !tapi-tbd-v2";
const TBD_V3_DOCUMENT_START: &str = "--- !tapi-tbd-v3";
const TBD_V4_DOCUMENT_START: &str = "--- !tapi-tbd";

/// Parse TBD records from a YAML stream.
///
/// Returns a series of parsed records contained in the stream.
pub fn parse_str(data: &str) -> Result<Vec<TBDRecord>, ParseError> {
    // serde_yaml doesn't support tags on documents with YAML streams
    // (https://github.com/dtolnay/serde-yaml/issues/147) because yaml-rust
    // doesn't do so (https://github.com/chyh1990/yaml-rust/issues/147). Our
    // extremely hacky and inefficient solution is to parse the stream once
    // using yaml_rust to ensure it is valid YAML. Then we do a manual pass
    // scanning for document markers (`---` and `...`) and corresponding TBD
    // tags. We then pair things up and feed each document into the serde_yaml
    // deserializer for the given type.

    let yamls = yaml_rust::YamlLoader::load_from_str(data)?;

    // We got valid YAML. That's a good sign. Proceed with document/tag scanning.

    let mut document_versions = vec![];

    for line in data.lines() {
        // Start of new YAML document.
        if line.starts_with("---") {
            let version = if line.starts_with(TBD_V2_DOCUMENT_START) {
                TBDVersion::V2
            } else if line.starts_with(TBD_V3_DOCUMENT_START) {
                TBDVersion::V3
            } else if line.starts_with(TBD_V4_DOCUMENT_START) {
                TBDVersion::V4
            } else {
                // Version 1 has no document tag.
                TBDVersion::V1
            };

            document_versions.push(version);
        }
    }

    // The initial document marker in a YAML file is optional. And the
    // `---` marker is a version 1 TBD. So if there is a count mismatch,
    // insert a version 1 at the beginning of the versions list.
    if document_versions.len() == yamls.len() - 1 {
        document_versions.insert(0, TBDVersion::V1);
    } else if document_versions.len() != yamls.len() {
        return Err(ParseError::DocumentCountMismatch);
    }

    let mut res = vec![];

    for (index, value) in yamls.iter().enumerate() {
        // TODO We could almost certainly avoid the YAML parsing round trip
        let mut s = String::new();
        yaml_rust::YamlEmitter::new(&mut s).dump(value).unwrap();

        res.push(match document_versions[index] {
            TBDVersion::V1 => TBDRecord::V1(serde_yaml::from_str(&s)?),
            TBDVersion::V2 => TBDRecord::V2(serde_yaml::from_str(&s)?),
            TBDVersion::V3 => TBDRecord::V3(serde_yaml::from_str(&s)?),
            TBDVersion::V4 => TBDRecord::V4(serde_yaml::from_str(&s)?),
        })
    }

    Ok(res)
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        rayon::prelude::*,
        tugger_apple::{
            find_command_line_tools_sdks, find_developer_sdks,
            find_system_xcode_developer_directories,
        },
    };

    #[test]
    fn test_parse_apple_sdk_tbds() {
        // This will find older Xcode versions and their SDKs when run in GitHub
        // Actions. That gives us extreme test coverage of real world .tbd files.
        let mut sdks = find_system_xcode_developer_directories()
            .unwrap()
            .into_iter()
            .map(|p| find_developer_sdks(&p).unwrap())
            .flatten()
            .collect::<Vec<_>>();

        if let Some(extra) = find_command_line_tools_sdks().unwrap() {
            sdks.extend(extra);
        }

        // Filter out symlinked SDKs to avoid duplicates.
        let sdks = sdks
            .into_iter()
            .filter(|sdk| !sdk.is_symlink)
            .collect::<Vec<_>>();

        sdks.into_par_iter().for_each(|sdk| {
            for entry in walkdir::WalkDir::new(&sdk.path) {
                let entry = entry.unwrap();

                let file_name = entry.file_name().to_string_lossy();
                if file_name.ends_with(".tbd") {
                    eprintln!("parsing {}", entry.path().display());
                    let data = std::fs::read(&entry.path()).unwrap();
                    let data = String::from_utf8(data).unwrap();

                    parse_str(&data).unwrap();
                }
            }
        });
    }
}