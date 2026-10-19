//! The vendored libraries an HTML artifact may use. They are served from the content origin at `/_lib/<file>`
//! (the web build copies them from `apps/web/node_modules` at the versions `apps/web/package.json` pins, and a test
//! checks the two lists agree), so artifacts work offline and never reach a third-party CDN. More are added on
//! request, here and in `package.json` together.

/// One vendored library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Lib {
    /// The import specifier an artifact names (`import * as d3 from "d3"`).
    pub name: &'static str,
    /// The npm package and its pinned version.
    pub package: &'static str,
    pub version: &'static str,
    /// The file inside the package that is served.
    pub source: &'static str,
    /// Its name under `/_lib/`.
    pub file: &'static str,
    /// The global the file defines, which compiled imports read. Empty for Tailwind, which only scans classes.
    pub global: &'static str,
    /// Libraries that must load first.
    pub needs: &'static [&'static str],
}

pub const LIBS: &[Lib] = &[
    Lib {
        name: "react",
        package: "react",
        version: "18.3.1",
        source: "umd/react.production.min.js",
        file: "react.js",
        global: "React",
        needs: &[],
    },
    Lib {
        name: "react-dom",
        package: "react-dom",
        version: "18.3.1",
        source: "umd/react-dom.production.min.js",
        file: "react-dom.js",
        global: "ReactDOM",
        needs: &["react"],
    },
    Lib {
        name: "prop-types",
        package: "prop-types",
        version: "15.8.1",
        source: "prop-types.min.js",
        file: "prop-types.js",
        global: "PropTypes",
        needs: &[],
    },
    Lib {
        name: "recharts",
        package: "recharts",
        version: "2.15.4",
        source: "umd/Recharts.js",
        file: "recharts.js",
        global: "Recharts",
        needs: &["react", "react-dom", "prop-types"],
    },
    Lib {
        name: "d3",
        package: "d3",
        version: "7.9.0",
        source: "dist/d3.min.js",
        file: "d3.js",
        global: "d3",
        needs: &[],
    },
    Lib {
        name: "three",
        package: "three",
        version: "0.160.1",
        source: "build/three.min.js",
        file: "three.js",
        global: "THREE",
        needs: &[],
    },
    Lib {
        name: "lodash",
        package: "lodash",
        version: "4.17.21",
        source: "lodash.min.js",
        file: "lodash.js",
        global: "_",
        needs: &[],
    },
    Lib {
        name: "papaparse",
        package: "papaparse",
        version: "5.5.3",
        source: "papaparse.min.js",
        file: "papaparse.js",
        global: "Papa",
        needs: &[],
    },
    Lib {
        name: "lucide",
        package: "lucide",
        version: "1.55.0",
        source: "dist/umd/lucide.min.js",
        file: "lucide.js",
        global: "lucide",
        needs: &[],
    },
    Lib {
        name: "tailwindcss",
        package: "@tailwindcss/browser",
        version: "4.1.13",
        source: "dist/index.global.js",
        file: "tailwind.js",
        global: "",
        needs: &[],
    },
];

/// The library an import specifier names: the package itself or a path inside it (`react-dom/client`,
/// `lodash/debounce`).
pub fn lib(specifier: &str) -> Option<&'static Lib> {
    let base = if let Some(rest) = specifier.strip_prefix('@') {
        let mut parts = rest.splitn(3, '/');
        match (parts.next(), parts.next()) {
            (Some(scope), Some(name)) => format!("@{scope}/{name}"),
            _ => specifier.to_string(),
        }
    } else {
        specifier.split('/').next().unwrap_or(specifier).to_string()
    };
    LIBS.iter().find(|l| l.name == base || l.package == base)
}

/// The library served as `/_lib/<file>`.
pub fn by_file(file: &str) -> Option<&'static Lib> {
    LIBS.iter().find(|l| l.file == file)
}

/// `names` and everything they need, each once, in an order that loads dependencies first.
pub fn closure(names: &[&str]) -> Vec<&'static str> {
    fn visit(name: &str, out: &mut Vec<&'static str>) {
        if let Some(l) = LIBS.iter().find(|l| l.name == name) {
            if out.contains(&l.name) {
                return;
            }
            for n in l.needs {
                visit(n, out);
            }
            out.push(l.name);
        }
    }
    let mut out = vec![];
    for n in names {
        visit(n, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specifiers_resolve() {
        assert_eq!(lib("react").unwrap().global, "React");
        assert_eq!(lib("react-dom/client").unwrap().name, "react-dom");
        assert_eq!(lib("lodash/debounce").unwrap().name, "lodash");
        assert_eq!(lib("@tailwindcss/browser").unwrap().name, "tailwindcss");
        assert!(lib("left-pad").is_none());
    }

    #[test]
    fn closure_loads_dependencies_first() {
        assert_eq!(
            closure(&["recharts"]),
            vec!["react", "react-dom", "prop-types", "recharts"]
        );
        assert_eq!(
            closure(&["d3", "react", "react-dom"]),
            vec!["d3", "react", "react-dom"]
        );
    }
}
