//! The compiler's output for the artifact scripts it must accept, and the errors it must give for the ones it refuses.

use apz_artifact::compile;

#[test]
fn a_tsx_component_loses_its_types_and_reads_react_from_the_page() {
    let source = r#"import React, {useState} from "react";
interface Props { label: string; count?: number }
export default function Counter({label, count = 0}: Props) {
  const [n, setN] = useState<number>(count);
  return <button onClick={() => setN(n + 1)}>{label}: {n}</button>;
}
"#;
    let js = compile("Counter.tsx", source).expect("compiles");
    assert!(js.contains("React.createElement"), "{js}");
    assert!(js.contains("const React = window.React;"), "{js}");
    assert!(js.contains("const {useState} = window.React;"), "{js}");
    assert!(!js.contains("interface"), "{js}");
    assert!(!js.contains("import "), "{js}");
}

#[test]
fn a_recharts_import_reads_the_recharts_global() {
    let source = r#"import {LineChart as LC, Line} from "recharts";
export const Chart = () => <LC data={[]}><Line dataKey="x" /></LC>;
"#;
    let js = compile("Chart.jsx", source).expect("compiles");
    assert!(
        js.contains("const {LineChart: LC, Line} = window.Recharts;"),
        "{js}"
    );
}

#[test]
fn a_namespace_import_of_d3_binds_the_global() {
    let js = compile(
        "chart.js",
        "import * as d3 from \"d3\";\nexport const xs = d3.range(3);\n",
    )
    .expect("compiles");
    assert!(js.contains("const d3 = window.d3;"), "{js}");
    assert!(js.contains("d3.range(3)"), "{js}");
}

#[test]
fn a_relative_import_is_kept() {
    let js = compile(
        "App.tsx",
        "import {helper} from \"./helper.js\";\nexport const y = helper();\n",
    )
    .expect("compiles");
    assert!(js.contains("from \"./helper.js\""), "{js}");
}

#[test]
fn an_unvendored_package_is_refused_by_name() {
    let problems = compile(
        "App.tsx",
        "import x from \"left-pad\";\nexport const y = x;\n",
    )
    .expect_err("refused");
    assert!(
        problems.iter().any(|p| p.contains("left-pad")),
        "{problems:?}"
    );
}

#[test]
fn a_network_import_is_refused() {
    let problems =
        compile("App.tsx", "import x from \"https://example.com/x.js\";\n").expect_err("refused");
    assert!(
        problems.iter().any(|p| p.contains("network")),
        "{problems:?}"
    );
}

#[test]
fn a_syntax_error_names_its_line() {
    let problems = compile("App.tsx", "const a = 1;\nconst b = ;\n").expect_err("refused");
    assert!(problems.iter().any(|p| p.starts_with("2:")), "{problems:?}");
}

#[test]
fn a_javascript_file_with_only_vendored_imports_is_rewritten() {
    let source = "import React from \"react\";\nimport {createRoot} from \"react-dom/client\";\nconsole.log(React, createRoot);\n";
    let js = compile("main.js", source).expect("compiles");
    assert!(js.contains("const React = window.React;"), "{js}");
    assert!(js.contains("const {createRoot} = window.ReactDOM;"), "{js}");
    assert!(!js.contains("import "), "{js}");
}
