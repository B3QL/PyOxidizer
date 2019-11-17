// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use itertools::Itertools;
use slog::warn;
use starlark::environment::Environment;
use starlark::values::{
    default_compare, RuntimeError, TypedValue, Value, ValueError, ValueResult,
    INCORRECT_PARAMETER_TYPE_ERROR_CODE,
};
use starlark::{
    any, immutable, not_supported, starlark_fun, starlark_module, starlark_signature,
    starlark_signature_extraction, starlark_signatures,
};
use std::any::Any;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::env::{optional_str_arg, required_str_arg};
use super::python_resource::PythonSourceModule;
use crate::app_packaging::environment::EnvironmentContext;
use crate::py_packaging::distribution::{
    resolve_parsed_distribution, ParsedPythonDistribution, PythonDistributionLocation,
};
use crate::python_distributions::CPYTHON_BY_TRIPLE;

#[derive(Debug, Clone)]
pub struct PythonDistribution {
    pub source: PythonDistributionLocation,

    dest_dir: PathBuf,

    distribution: Option<ParsedPythonDistribution>,
}

impl PythonDistribution {
    fn from_location(location: PythonDistributionLocation, dest_dir: &Path) -> PythonDistribution {
        PythonDistribution {
            source: location,
            dest_dir: dest_dir.to_path_buf(),
            distribution: None,
        }
    }

    fn ensure_distribution_resolved(&mut self, logger: &slog::Logger) {
        if self.distribution.is_some() {
            return;
        }

        let dist = resolve_parsed_distribution(logger, &self.source, &self.dest_dir).unwrap();
        warn!(logger, "distribution info: {:#?}", dist.as_minimal_info());

        self.distribution = Some(dist);
    }
}

impl TypedValue for PythonDistribution {
    immutable!();
    any!();
    not_supported!(binop);
    not_supported!(container);
    not_supported!(function);
    not_supported!(get_hash);
    not_supported!(to_int);

    fn to_str(&self) -> String {
        format!("PythonDistribution<{:#?}>", self.source)
    }

    fn to_repr(&self) -> String {
        self.to_str()
    }

    fn get_type(&self) -> &'static str {
        "PythonDistribution"
    }

    fn to_bool(&self) -> bool {
        true
    }

    fn compare(&self, other: &dyn TypedValue, _recursion: u32) -> Result<Ordering, ValueError> {
        default_compare(self, other)
    }
}

starlark_module! { python_distribution_module =>
    #[allow(non_snake_case)]
    PythonDistribution(env env, sha256, local_path=None, url=None) {
        required_str_arg("sha256", &sha256)?;
        optional_str_arg("local_path", &local_path)?;
        optional_str_arg("url", &url)?;

        if local_path.get_type() != "NoneType" && url.get_type() != "NoneType" {
            return Err(RuntimeError {
                code: INCORRECT_PARAMETER_TYPE_ERROR_CODE,
                message: "cannot define both local_path and url".to_string(),
                label: "cannot define both local_path and url".to_string(),
            }.into());
        }

        let distribution = if local_path.get_type() != "NoneType" {
            PythonDistributionLocation::Local {
                local_path: local_path.to_string(),
                sha256: sha256.to_string(),
            }
        } else {
            PythonDistributionLocation::Url {
                url: url.to_string(),
                sha256: sha256.to_string(),
            }
        };

        let context = env.get("CONTEXT").expect("CONTEXT not defined");
        let dest_dir = context.downcast_apply(|x: &EnvironmentContext| x.python_distributions_path.clone());

        Ok(Value::new(PythonDistribution::from_location(distribution, &dest_dir)))
    }

    PythonDistribution.source_modules(env env, this) {
        let context = env.get("CONTEXT").expect("CONTEXT not defined");

        let logger = context.downcast_apply(|x: &EnvironmentContext| x.logger.clone());

        Ok(Value::from(this.downcast_apply_mut(|dist: &mut PythonDistribution| {
            dist.ensure_distribution_resolved(&logger);

            dist.distribution.as_ref().unwrap().source_modules().iter().map(|module| {
                Value::new(PythonSourceModule { module: module.clone() })
            }).collect_vec()
        })))
    }

    default_python_distribution(env env, build_target=None) {
        let build_target = match build_target.get_type() {
            "NoneType" => env.get("BUILD_TARGET").unwrap().to_string(),
            "string" => build_target.to_string(),
            t => {
                return Err(ValueError::TypeNotX {
                    object_type: t.to_string(),
                    op: "str".to_string(),
                })
            }
        };

        match CPYTHON_BY_TRIPLE.get(&build_target) {
            Some(dist) => {
                let distribution = PythonDistributionLocation::Url {
                    url: dist.url.clone(),
                    sha256: dist.sha256.clone(),
                };

                let context = env.get("CONTEXT").expect("CONTEXT not defined");
                let dest_dir = context.downcast_apply(|x: &EnvironmentContext| x.python_distributions_path.clone());

                Ok(Value::new(PythonDistribution::from_location(distribution, &dest_dir)))
            }
            None => Err(ValueError::Runtime(RuntimeError {
                code: "no_default_distribution",
                message: format!("could not find default Python distribution for {}", build_target),
                label: "build_target".to_string(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    #[test]
    fn test_default_python_distribution() {
        let dist = starlark_ok("default_python_distribution()");
        assert_eq!(dist.get_type(), "PythonDistribution");

        let host_distribution = CPYTHON_BY_TRIPLE
            .get(crate::app_packaging::repackage::HOST)
            .unwrap();

        let wanted = PythonDistributionLocation::Url {
            url: host_distribution.url.clone(),
            sha256: host_distribution.sha256.clone(),
        };

        dist.downcast_apply(|x: &PythonDistribution| assert_eq!(x.source, wanted));
    }

    #[test]
    fn test_default_python_distribution_bad_arg() {
        let err = starlark_nok("default_python_distribution(False)");
        assert_eq!(err.message, "The type 'bool' is not str");
    }

    #[test]
    fn test_python_distribution_no_args() {
        let err = starlark_nok("PythonDistribution()");
        assert!(err.message.starts_with("Missing parameter sha256"));
    }

    #[test]
    fn test_python_distribution_multiple_args() {
        let err = starlark_nok(
            "PythonDistribution('sha256', url='url_value', local_path='local_path_value')",
        );
        assert_eq!(err.message, "cannot define both local_path and url");
    }

    #[test]
    fn test_python_distribution_url() {
        let dist = starlark_ok("PythonDistribution('sha256', url='some_url')");
        let wanted = PythonDistributionLocation::Url {
            url: "some_url".to_string(),
            sha256: "sha256".to_string(),
        };

        dist.downcast_apply(|x: &PythonDistribution| assert_eq!(x.source, wanted));
    }

    #[test]
    fn test_python_distribution_local_path() {
        let dist = starlark_ok("PythonDistribution('sha256', local_path='some_path')");
        let wanted = PythonDistributionLocation::Local {
            local_path: "some_path".to_string(),
            sha256: "sha256".to_string(),
        };

        dist.downcast_apply(|x: &PythonDistribution| assert_eq!(x.source, wanted));
    }

    #[test]
    fn test_source_modules() {
        let mods = starlark_ok("default_python_distribution().source_modules()");
        assert_eq!(mods.get_type(), "list");
    }
}
