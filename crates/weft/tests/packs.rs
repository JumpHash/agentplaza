//! Every pack in `crates/weft/packs` loads, its query compiles against its grammar, and it finds the
//! definitions in a small sample. Packs load only when a file needs them, and unload when idle.

mod common;

use std::time::Duration;

use common::plaza;
use weft::derived::Derived;

const SAMPLES: &[(&str, &str, &str, &[&str])] = &[
    (
        "rust",
        "src/lib.rs",
        "struct Node { id: u64 }\nimpl Node {\n    fn wake(&self) {}\n}\n",
        &["struct Node", "impl Node › fn wake"],
    ),
    (
        "typescript",
        "src/store.ts",
        "export interface Task { id: string }\nexport function load(): Task[] { return []; }\nclass Store { save() {} }\n",
        &["trait Task", "fn load", "type Store › fn save"],
    ),
    (
        "tsx",
        "src/App.tsx",
        "export function App() { return <div/>; }\n",
        &["fn App"],
    ),
    (
        "javascript",
        "app.js",
        "const KINDS = ['a'];\nfunction start() {}\nclass Plaza { open() {} }\n",
        &["const KINDS", "fn start", "type Plaza › fn open"],
    ),
    (
        "python",
        "tool.py",
        "class Runner:\n    def run(self):\n        pass\n\ndef main():\n    Runner().run()\n",
        &["type Runner › fn run", "fn main"],
    ),
    (
        "go",
        "main.go",
        "package main\n\ntype Node struct{}\n\nfunc (n Node) Wake() {}\n\nfunc main() {}\n",
        &["fn main"],
    ),
    (
        "java",
        "Main.java",
        "class Main {\n  void run() {}\n}\n",
        &["type Main › fn run"],
    ),
    (
        "c",
        "main.c",
        "int add(int a, int b) { return a + b; }\n",
        &["fn add"],
    ),
    (
        "cpp",
        "main.cpp",
        "int add(int a, int b) { return a + b; }\n",
        &["fn add"],
    ),
    (
        "c_sharp",
        "Main.cs",
        "class Main { void Run() {} }\n",
        &["type Main › fn Run"],
    ),
    (
        "ruby",
        "app.rb",
        "class Plaza\n  def open\n  end\nend\n",
        &["type Plaza › fn open"],
    ),
    (
        "php",
        "index.php",
        "<?php\nfunction start() {}\nclass Plaza { function open() {} }\n",
        &["fn start", "type Plaza › fn open"],
    ),
    (
        "css",
        "app.css",
        ":root { --accent: #0a7; }\n.card { color: var(--accent); }\n",
        &[".card"],
    ),
    (
        "json",
        "package.json",
        "{\"name\": \"plaza\", \"scripts\": {\"build\": \"vite\"}}\n",
        &["name", "scripts › key build"],
    ),
    (
        "toml",
        "Cargo.toml",
        "[package]\nname = \"weft\"\n\n[dependencies]\nserde = \"1\"\n",
        &["package", "dependencies"],
    ),
    (
        "yaml",
        "ci.yml",
        "name: ci\njobs:\n  test:\n    runs-on: ubuntu\n",
        &["name", "jobs"],
    ),
    (
        "bash",
        "build.sh",
        "#!/bin/bash\nbuild() {\n  cargo build\n}\n",
        &["fn build"],
    ),
    (
        "html",
        "index.html",
        "<html><body><main id=\"app\"></main></body></html>\n",
        &["app"],
    ),
    (
        "markdown",
        "README.md",
        "# Weft\n\n## Install\n\n- Download the binary.\n",
        &["section Weft › section Install"],
    ),
    (
        "kotlin",
        "Plaza.kt",
        "class Plaza {\n    fun open() {}\n}\n",
        &["type Plaza", "type Plaza › fn open"],
    ),
    (
        "powershell",
        "Plaza.ps1",
        "class Plaza {\n    [void] Open() {}\n}\n",
        &["type Plaza", "type Plaza › fn Open"],
    ),
];

#[test]
fn every_pack_loads_and_finds_definitions() {
    let p = plaza("packs", &[]);
    let names: Vec<&str> = SAMPLES.iter().map(|s| s.0).collect();
    p.add_packs(&names);
    let derived = Derived::new(p.dir.join("derived"));
    assert!(
        derived.runtime.loaded().is_empty(),
        "nothing is loaded up front"
    );
    let mut problems = Vec::new();
    for (pack, path, text, want) in SAMPLES {
        let parsed = derived.parse_bytes(&p.store, path, text.as_bytes(), None);
        let got: Vec<&str> = parsed
            .symbols
            .iter()
            .map(|s| s.qualified.as_str())
            .collect();
        if parsed.pack.as_deref() != Some(*pack) {
            problems.push(format!("{path}: pack {:?}, wanted {pack}", parsed.pack));
            continue;
        }
        for w in *want {
            if !got.iter().any(|g| g.ends_with(w)) {
                problems.push(format!("{pack}: no `{w}` in {got:?}"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert_eq!(derived.runtime.loaded().len(), SAMPLES.len());
    // A file no pack knows is not parsed, and loads nothing.
    let none = derived.parse_bytes(&p.store, "notes.txt", b"plain text", None);
    assert!(none.pack.is_none() && none.symbols.is_empty());
    assert_eq!(derived.runtime.unload_idle(Duration::ZERO), SAMPLES.len());
    assert!(derived.runtime.loaded().is_empty());
    std::fs::remove_dir_all(&p.dir).ok();
}
