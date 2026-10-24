//! Reading a page over DevTools (PLAN 8.2's web target, 22.9): the accessibility tree (`Accessibility.getFullAXTree`) joined
//! with the facts of each node's element (`DOM.resolveNode` then a function run on it, `Runtime.callFunctionOn`). The
//! browser's session is behind [`Devtools`], so the probe is tested over a scripted browser and runs over the web target's
//! session in production.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::tree::{AxNode, Element, Snapshot};

/// The most elements one snapshot reads: a page's first elements in its tree's order.
pub const MAX_ELEMENTS: usize = 1500;

/// The DevTools commands a check needs from a browser's page.
pub trait Devtools {
    /// One DevTools command to the page: its answer's `result`, or the browser's error.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;
    /// Loads `url` in the page and waits until it has loaded.
    fn load(&mut self, url: &str) -> Result<(), String>;
}

/// The facts of one element, run on the element (`this` is the node: an element, or a text node whose element is its
/// parent). Returns the fields of [`Element`] except its key.
const FACTS: &str = r#"function () {
  const el = this.nodeType === 1 ? this : this.parentElement;
  if (!el) return null;
  const cs = getComputedStyle(el);
  const r = el.getBoundingClientRect();
  const visible = cs.display !== 'none' && cs.visibility !== 'hidden' && r.width > 0 && r.height > 0;
  const text = Array.from(el.childNodes).filter(n => n.nodeType === 3).map(n => n.textContent).join(' ').replace(/\s+/g, ' ').trim();
  const parse = (s) => {
    const m = /rgba?\(([^)]*)\)/.exec(s || '');
    if (!m) return null;
    const p = m[1].split(/[\s,\/]+/).filter(Boolean).map(Number);
    return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1];
  };
  const layers = [];
  let unknown = false;
  for (let n = el; n && n.nodeType === 1; n = n.parentElement) {
    const s = getComputedStyle(n);
    if (s.backgroundImage && s.backgroundImage !== 'none') { unknown = true; break; }
    const c = parse(s.backgroundColor);
    if (c && c[3] > 0) { layers.push(c); if (c[3] >= 1) break; }
  }
  let bg = [255, 255, 255];
  if (unknown) {
    bg = null;
  } else {
    for (let i = layers.length - 1; i >= 0; i--) {
      const c = layers[i];
      bg = bg.map((v, k) => c[k] * c[3] + v * (1 - c[3]));
    }
  }
  let fg = parse(cs.color);
  if (fg && fg[3] < 1) {
    fg = bg ? fg.slice(0, 3).map((v, k) => v * fg[3] + bg[k] * (1 - fg[3])) : null;
  } else if (fg) {
    fg = fg.slice(0, 3);
  }
  const ti = el.hasAttribute('tabindex') ? Number(el.getAttribute('tabindex')) : null;
  const base = el.matches('a[href],button,input,select,textarea,summary,iframe,[contenteditable="true"]') && !el.disabled;
  return {
    tag: el.tagName.toLowerCase(),
    id: el.id || '',
    text: text,
    tabindex: ti,
    focusable: ti === null ? base : ti >= 0,
    visible: visible,
    rect: [r.left + window.scrollX, r.top + window.scrollY, r.width, r.height],
    fg: fg && bg ? fg.slice(0, 3) : null,
    bg: bg,
    font_px: parseFloat(cs.fontSize),
    bold: Number(cs.fontWeight) >= 700,
    overflow_x: cs.overflowX,
    scroll_width: el.scrollWidth,
    client_width: el.clientWidth
  };
}"#;

/// A string field of an accessibility node: its `value`.
fn field(node: &Value, name: &str) -> String {
    node[name]["value"].as_str().unwrap_or_default().to_string()
}

/// The snapshot of the page the browser has loaded.
pub fn snapshot(dt: &mut dyn Devtools) -> Result<Snapshot, String> {
    let lang = dt.call(
        "Runtime.evaluate",
        json!({ "expression": "document.documentElement.getAttribute('lang') || ''", "returnByValue": true }),
    )?;
    let mut snapshot = Snapshot {
        lang: lang["result"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        ..Snapshot::default()
    };
    let tree = dt.call("Accessibility.getFullAXTree", json!({}))?;
    let mut backends = HashSet::new();
    let mut facts = HashSet::new();
    for node in tree["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let backend = node["backendDOMNodeId"].as_u64();
        let key = backend.map(|b| format!("dom-{b}")).unwrap_or_default();
        snapshot.nodes.push(AxNode {
            key: key.clone(),
            role: field(node, "role"),
            name: field(node, "name"),
            ignored: node["ignored"].as_bool().unwrap_or(false),
        });
        let Some(backend) = backend else {
            continue;
        };
        if snapshot.elements.len() >= MAX_ELEMENTS || !backends.insert(backend) {
            continue;
        }
        let Some(mut element) = element(dt, backend) else {
            continue;
        };
        element.key = key;
        // A text node and its element are read twice: the element once.
        let identity = format!(
            "{}|{:?}|{}|{}",
            element.tag, element.rect, element.text, element.id
        );
        if facts.insert(identity) {
            snapshot.elements.push(element);
        }
    }
    Ok(snapshot)
}

/// The facts of the node `backend`, or None when the browser has no element for it (an ignored node with no DOM).
fn element(dt: &mut dyn Devtools, backend: u64) -> Option<Element> {
    let resolved = dt
        .call("DOM.resolveNode", json!({ "backendNodeId": backend }))
        .ok()?;
    let object = resolved["object"]["objectId"].as_str()?.to_string();
    let answer = dt
        .call(
            "Runtime.callFunctionOn",
            json!({ "objectId": object, "functionDeclaration": FACTS, "returnByValue": true }),
        )
        .ok()?;
    if answer.get("exceptionDetails").is_some() {
        return None;
    }
    serde_json::from_value(answer["result"]["value"].clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::audit;
    use crate::layout::overflow;

    /// A browser that answers from a fixed page: its tree and the facts of its nodes.
    struct Scripted {
        tree: Value,
        facts: Vec<(u64, Value)>,
        calls: Vec<String>,
    }

    impl Devtools for Scripted {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
            self.calls.push(method.to_string());
            match method {
                "Runtime.evaluate" => Ok(json!({ "result": { "value": "" } })),
                "Accessibility.getFullAXTree" => Ok(self.tree.clone()),
                "DOM.resolveNode" => {
                    let backend = params["backendNodeId"].as_u64().unwrap();
                    if self.facts.iter().any(|(b, _)| *b == backend) {
                        Ok(json!({ "object": { "objectId": backend.to_string() } }))
                    } else {
                        Err("No node with given id found".into())
                    }
                }
                "Runtime.callFunctionOn" => {
                    let backend: u64 = params["objectId"].as_str().unwrap().parse().unwrap();
                    let fact = self.facts.iter().find(|(b, _)| *b == backend).unwrap();
                    Ok(json!({ "result": { "value": fact.1.clone() } }))
                }
                other => Err(format!("{other} is not scripted")),
            }
        }

        fn load(&mut self, _url: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn fact(text: &str, tag: &str) -> Value {
        json!({
            "tag": tag, "id": "", "text": text, "tabindex": null, "focusable": false,
            "visible": true, "rect": [0.0, 0.0, 200.0, 20.0],
            "fg": [0.0, 0.0, 0.0], "bg": [255.0, 255.0, 255.0],
            "font_px": 16.0, "bold": false, "overflow_x": "visible",
            "scroll_width": 200.0, "client_width": 200.0
        })
    }

    #[test]
    fn a_scripted_page_is_read_into_a_snapshot_that_the_rules_judge() {
        // A button with no name (backend 2), a heading (3) whose text is faint (fg #bbb) and a nowrap box (4) whose text is
        // wider than it. Backend 9 is an ignored node with no DOM: it is kept in the tree, read as no element.
        let mut faint = fact("Faint heading", "h2");
        faint["fg"] = json!([187.0, 187.0, 187.0]);
        let mut wide = fact("A line that runs past its box", "div");
        wide["scroll_width"] = json!(340.0);
        wide["client_width"] = json!(160.0);
        let mut browser = Scripted {
            tree: json!({ "nodes": [
                { "backendDOMNodeId": 1, "role": {"value": "RootWebArea"}, "name": {"value": "Shop"}, "ignored": false },
                { "backendDOMNodeId": 2, "role": {"value": "button"}, "name": {"value": ""}, "ignored": false },
                { "backendDOMNodeId": 3, "role": {"value": "heading"}, "name": {"value": "Faint heading"}, "ignored": false },
                { "backendDOMNodeId": 4, "role": {"value": "generic"}, "name": {"value": ""}, "ignored": true },
                { "backendDOMNodeId": 9, "role": {"value": "none"}, "ignored": true },
            ]}),
            facts: vec![(2, fact("", "button")), (3, faint), (4, wide)],
            calls: Vec::new(),
        };
        let snapshot = snapshot(&mut browser).unwrap();
        assert_eq!(snapshot.nodes.len(), 5, "every tree node is kept");
        assert_eq!(
            snapshot.elements.len(),
            3,
            "the node with no DOM is not an element"
        );
        assert!(snapshot.lang.is_empty());
        let found: Vec<&str> = audit(&snapshot)
            .into_iter()
            .chain(overflow(&snapshot))
            .map(|i| i.rule)
            .collect();
        assert_eq!(
            found,
            vec![
                "control-name",
                "html-has-lang",
                "color-contrast",
                "text-overflow"
            ]
        );
        assert!(
            browser
                .calls
                .contains(&"Runtime.callFunctionOn".to_string())
        );
    }

    #[test]
    fn a_text_node_and_its_element_are_one_element() {
        // The text node (backend 5) and its paragraph (backend 6) read the same element: one element in the snapshot.
        let mut browser = Scripted {
            tree: json!({ "nodes": [
                { "backendDOMNodeId": 5, "role": {"value": "StaticText"}, "name": {"value": "Hi"}, "ignored": false },
                { "backendDOMNodeId": 6, "role": {"value": "paragraph"}, "name": {"value": ""}, "ignored": false },
            ]}),
            facts: vec![(5, fact("Hi", "p")), (6, fact("Hi", "p"))],
            calls: Vec::new(),
        };
        let snapshot = snapshot(&mut browser).unwrap();
        assert_eq!(snapshot.elements.len(), 1);
        assert_eq!(snapshot.elements[0].key, "dom-5");
    }
}
