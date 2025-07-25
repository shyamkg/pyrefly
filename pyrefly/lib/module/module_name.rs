/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use std::ffi::OsString;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;
use std::path::Path;

use dupe::Dupe;
use equivalent::Equivalent;
use pyrefly_util::visit::Visit;
use pyrefly_util::visit::VisitMut;
use ruff_python_ast::name::Name;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use static_interner::Intern;
use static_interner::Interner;
use thiserror::Error;

use crate::python::dunder;

static MODULE_NAME_INTERNER: Interner<String> = Interner::new();

/// The name of a python module. Examples: `foo.bar.baz`, `.foo.bar`.
#[derive(Clone, Dupe, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModuleName(Intern<String>);

impl<To: 'static> Visit<To> for ModuleName {
    const RECURSE_CONTAINS: bool = false;
    fn recurse<'a>(&'a self, _: &mut dyn FnMut(&'a To)) {}
}

impl<To: 'static> VisitMut<To> for ModuleName {
    const RECURSE_CONTAINS: bool = false;
    fn recurse_mut(&mut self, _: &mut dyn FnMut(&mut To)) {}
}

impl Serialize for ModuleName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ModuleName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s: &str = Deserialize::deserialize(deserializer)?;
        Ok(ModuleName::from_str(s))
    }
}

impl Display for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            write!(f, ".")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

#[derive(Debug, Error)]
enum PathConversionError {
    #[error("invalid source file extension (file name: `{file_name}`")]
    InvalidExtension { file_name: String },
    #[error("path component is not UTF-8 encoded: `{component:?}`")]
    ComponentNotUTF8 { component: OsString },
}

impl Debug for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_tuple("ModuleName");
        f.field(&self.as_str());
        f.finish()
    }
}

#[derive(Hash, Eq, PartialEq)]
struct StrRef<'a>(&'a str);

impl Equivalent<String> for StrRef<'_> {
    fn equivalent(&self, key: &String) -> bool {
        self.0 == key
    }
}

impl From<StrRef<'_>> for String {
    fn from(value: StrRef<'_>) -> Self {
        value.0.to_owned()
    }
}

impl ModuleName {
    pub fn builtins() -> Self {
        Self::from_str("builtins")
    }

    pub fn typing() -> Self {
        Self::from_str("typing")
    }

    pub fn typing_extensions() -> Self {
        Self::from_str("typing_extensions")
    }

    pub fn types() -> Self {
        Self::from_str("types")
    }

    pub fn warnings() -> Self {
        Self::from_str("warnings")
    }

    pub fn collections() -> Self {
        Self::from_str("collections")
    }

    pub fn enum_() -> Self {
        Self::from_str("enum")
    }

    pub fn abc() -> Self {
        Self::from_str("abc")
    }

    pub fn dataclasses() -> Self {
        Self::from_str("dataclasses")
    }

    pub fn functools() -> Self {
        Self::from_str("functools")
    }

    pub fn type_checker_internals() -> Self {
        Self::from_str("_typeshed._type_checker_internals")
    }

    /// The "unknown" module name, which corresponds to `__unknown__`.
    /// Used for files directly opened or passed on the command line which aren't on the search path.
    pub fn unknown() -> Self {
        Self::from_str("__unknown__")
    }

    pub fn from_str(x: &str) -> Self {
        ModuleName(MODULE_NAME_INTERNER.intern(StrRef(x)))
    }

    pub fn from_string(x: String) -> Self {
        ModuleName(MODULE_NAME_INTERNER.intern(x))
    }

    pub fn from_name(x: &Name) -> Self {
        Self::from_str(x)
    }

    pub fn from_parts(parts: impl IntoIterator<Item = impl Display + AsRef<str>>) -> Self {
        Self::from_string(itertools::join(parts, "."))
    }

    pub fn from_relative_path(path: &Path) -> anyhow::Result<Self> {
        let mut components = Vec::new();
        for raw_component in path.components() {
            if let Some(component) = raw_component.as_os_str().to_str() {
                components.push(component)
            } else {
                return Err(anyhow::anyhow!(PathConversionError::ComponentNotUTF8 {
                    component: raw_component.as_os_str().to_owned(),
                }));
            }
        }
        let last_element = components.pop();
        match last_element {
            None => {}
            Some(file_name) => {
                let splits: Vec<&str> = file_name.rsplitn(2, '.').collect();
                if splits.len() != 2 || !(splits[0] == "py" || splits[0] == "pyi") {
                    return Err(anyhow::anyhow!(PathConversionError::InvalidExtension {
                        file_name: file_name.to_owned(),
                    }));
                }
                if splits[1] != dunder::INIT {
                    components.push(splits[1])
                }
            }
        }
        Ok(ModuleName::from_parts(components))
    }

    pub fn append(self, name: &Name) -> Self {
        Self::from_string(format!("{}.{}", self.as_str(), name))
    }

    /// Create a new ModuleName instance based off the current instance, with:
    /// - specified number of dots removed
    /// - specified suffix appended
    ///
    /// * `is_init` - Whether the current module is an __init__.py file
    /// * `dots` - The number of dots to remove
    /// * `suffix` - The suffix to append to the current module
    pub fn new_maybe_relative(
        self,
        is_init: bool,
        mut dots: u32,
        suffix: Option<&Name>,
    ) -> Option<Self> {
        if dots == 0
            && let Some(s) = suffix
        {
            return Some(ModuleName::from_name(s));
        }
        let mut components = self.components();
        if is_init {
            dots = dots.saturating_sub(1);
        }
        for _ in 0..dots {
            components.pop()?;
        }
        if let Some(suffix) = suffix {
            components.push(suffix.clone());
        }
        Some(ModuleName::from_parts(components))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn first_component(self) -> Name {
        match self.0.split_once('.') {
            None => Name::new(self.as_str()),
            Some(x) => Name::new(x.0),
        }
    }

    pub fn components(self) -> Vec<Name> {
        self.0.split('.').map(Name::new).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_component() {
        assert_eq!(
            ModuleName::from_str("a.b.c").first_component(),
            Name::new_static("a")
        );
        assert_eq!(
            ModuleName::from_str("a").first_component(),
            Name::new_static("a")
        );
    }

    #[test]
    fn test_relative() {
        let base = ModuleName::from_str("a.b.c");
        assert_eq!(
            base.new_maybe_relative(false, 0, Some(&Name::new_static("d")))
                .unwrap(),
            ModuleName::from_str("d")
        );
        assert_eq!(
            base.new_maybe_relative(false, 1, Some(&Name::new_static("d")))
                .unwrap(),
            ModuleName::from_str("a.b.d")
        );
        assert_eq!(
            base.new_maybe_relative(false, 2, Some(&Name::new_static("d")))
                .unwrap(),
            ModuleName::from_str("a.d")
        );
        assert_eq!(
            base.new_maybe_relative(false, 3, Some(&Name::new_static("d")))
                .unwrap(),
            ModuleName::from_str("d")
        );
        // TODO: This is wrong. The relative level 4 should be invalid
        assert_eq!(
            base.new_maybe_relative(false, 4, Some(&Name::new_static("d"))),
            None
        );
        assert_eq!(
            base.new_maybe_relative(false, 1, None).unwrap(),
            ModuleName::from_str("a.b")
        );
        assert_eq!(
            base.new_maybe_relative(false, 2, None).unwrap(),
            ModuleName::from_str("a")
        );
        assert_eq!(
            ModuleName::from_str("sys")
                .new_maybe_relative(true, 1, None)
                .unwrap(),
            ModuleName::from_str("sys")
        );
    }

    #[test]
    fn test_from_relative_path() {
        fn assert_module_name(path: &str, expected: &str) {
            assert_eq!(
                ModuleName::from_relative_path(Path::new(path)).unwrap(),
                ModuleName::from_str(expected)
            );
        }
        assert_module_name("foo.py", "foo");
        assert_module_name("foo.pyi", "foo");
        assert_module_name("foo/bar.py", "foo.bar");
        assert_module_name("foo/bar.pyi", "foo.bar");
        assert_module_name("foo/bar/__init__.py", "foo.bar");
        assert_module_name("foo/bar/__init__.pyi", "foo.bar");

        fn assert_conversion_error(path: &str) {
            assert!(ModuleName::from_relative_path(Path::new(path)).is_err());
        }
        assert_conversion_error("foo/bar.derp");
        assert_conversion_error("foo/bar/baz");
        assert_conversion_error("foo/bar/__init__.derp");
    }
}
