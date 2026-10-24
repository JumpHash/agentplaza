//! The desktop target on a scenario, through the loop (PLAN 19.3), against a fake station. The fake answers the station
//! tools as a daemon does (its UI tree, its pointer and keys, its picture), from a script of screens: the stand-in's. The
//! seeded bug is a Pay button that leads back to the cart; the fixed station leads to the receipt. Each operating system
//! runs the same scenario: the Linux and the Windows targets differ only in the station's facts.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{Value, json};

use super::Ruling;
use super::desktop::{Calls, Gui, LINUX, WINDOWS, kind_of};
use super::target::{Action, Element, Link, Screen, StandIn, Target, Tree, render};
use super::targets_test::{pay_scenario, press_pay, run};
use crate::recipes::model::{DesktopTarget, UserTest, WebTarget};

const WIDTH: i64 = 800;
const HEIGHT: i64 = 600;

fn el(id: &str, role: &str, label: &str) -> Element {
    Element {
        id: id.into(),
        role: role.into(),
        label: label.into(),
        value: String::new(),
    }
}

/// The cart and its receipt. Pay leads to the receipt, or to the cart again when `buggy`.
fn shop(buggy: bool) -> StandIn {
    StandIn {
        start: "cart".into(),
        latency_ms: 0,
        screens: vec![
            Screen {
                id: "cart".into(),
                title: "Cart".into(),
                elements: vec![el("pay", "button", "Pay"), el("note", "textbox", "Note")],
                on: vec![Link {
                    element: "pay".into(),
                    to: if buggy { "cart" } else { "receipt" }.into(),
                }],
            },
            Screen {
                id: "receipt".into(),
                title: "Receipt".into(),
                elements: vec![el("home", "link", "Back to shop")],
                on: vec![],
            },
        ],
    }
}

/// A station's daemon, scripted: its screens and what is typed into them, what it was sent, and its lease.
struct FakeStation {
    os: &'static str,
    online: bool,
    queued: bool,
    script: StandIn,
    at: RefCell<String>,
    values: RefCell<BTreeMap<String, String>>,
    focus: RefCell<Option<String>>,
    pointers: RefCell<Vec<(i64, i64)>>,
    keys: RefCell<Vec<String>>,
    released: Cell<u32>,
}

impl FakeStation {
    fn new(os: &'static str, script: StandIn) -> FakeStation {
        FakeStation {
            os,
            online: true,
            queued: false,
            at: RefCell::new(script.start.clone()),
            script,
            values: RefCell::new(BTreeMap::new()),
            focus: RefCell::new(None),
            pointers: RefCell::new(Vec::new()),
            keys: RefCell::new(Vec::new()),
            released: Cell::new(0),
        }
    }

    fn screen(&self) -> Screen {
        let at = self.at.borrow().clone();
        self.script
            .screens
            .iter()
            .find(|screen| screen.id == at)
            .cloned()
            .expect("the fake station is on a screen of its script")
    }

    /// The screen as the daemon's tree reads it: what the stand-in shows, with what is typed in.
    fn tree(&self) -> Tree {
        let screen = self.screen();
        let values = self.values.borrow();
        Tree {
            title: screen.title.clone(),
            elements: screen
                .elements
                .iter()
                .map(|e| Element {
                    value: values
                        .get(&e.id)
                        .cloned()
                        .unwrap_or_else(|| e.value.clone()),
                    ..e.clone()
                })
                .collect(),
        }
    }

    /// The box of the control at `index`: a column of 300 by 40 points, 50 apart, under the window's top.
    fn bounds(index: usize) -> [i64; 4] {
        [20, 20 + 50 * index as i64, 300, 40]
    }

    /// The window's UI node, as the daemon's `ui_tree` encodes it.
    fn node(&self) -> Value {
        let screen = self.screen();
        let tree = self.tree();
        let children: Vec<Value> = tree
            .elements
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let role = if e.role == "textbox" {
                    "text_field"
                } else {
                    e.role.as_str()
                };
                let actions: Vec<&str> = if e.role == "button" || e.role == "link" {
                    vec!["invoke"]
                } else {
                    vec![]
                };
                let value = if e.value.is_empty() {
                    Value::Null
                } else {
                    json!(e.value)
                };
                json!({
                    "id": i + 1,
                    "role": role,
                    "name": e.label,
                    "value": value,
                    "bounds": Self::bounds(i),
                    "actions": actions,
                    "children": [],
                })
            })
            .collect();
        json!({
            "id": 0,
            "role": "window",
            "name": screen.title,
            "value": null,
            "bounds": [0, 0, WIDTH, HEIGHT],
            "actions": [],
            "children": children,
        })
    }

    /// The control under the point, if any.
    fn element_at(&self, x: i64, y: i64) -> Option<usize> {
        (0..self.screen().elements.len()).find(|&i| {
            let [bx, by, bw, bh] = Self::bounds(i);
            x >= bx && x < bx + bw && y >= by && y < by + bh
        })
    }

    /// A pointer press on the control at `index`: a button or a link leads where its link says, a text field takes the
    /// focus.
    fn touch(&self, index: usize) {
        let screen = self.screen();
        let element = &screen.elements[index];
        *self.focus.borrow_mut() = None;
        match element.role.as_str() {
            "button" | "link" => {
                if let Some(link) = screen.on.iter().find(|l| l.element == element.id) {
                    *self.at.borrow_mut() = link.to.clone();
                }
            }
            "textbox" => *self.focus.borrow_mut() = Some(element.id.clone()),
            _ => {}
        }
    }
}

impl Calls for FakeStation {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String> {
        match tool {
            "station_list" => Ok(json!([{
                "id": "ws-1",
                "online": self.online,
                "facts": {"os": self.os},
            }])),
            "station_lease" => {
                if input["kind"] != "pixel" {
                    return Err("the fake station grants pixel leases".into());
                }
                let session = if self.queued { Value::Null } else { json!(9) };
                Ok(json!({
                    "id": "l1",
                    "station": "ws-1",
                    "agent": 0,
                    "kind": "pixel",
                    "session": session,
                    "state": if self.queued { "queued" } else { "active" },
                    "position": if self.queued { json!(1) } else { Value::Null },
                }))
            }
            "station_release" => {
                self.released.set(self.released.get() + 1);
                Ok(Value::Null)
            }
            "station_ui_tree" => Ok(self.node()),
            "station_screenshot" => {
                let bytes = render(&self.tree())?;
                Ok(json!({
                    "w": WIDTH,
                    "h": HEIGHT,
                    "format": "png",
                    "data_b64": base64::engine::general_purpose::STANDARD.encode(bytes),
                }))
            }
            "station_click" => {
                if input["scale"] != 1.0 {
                    return Err("the fake station takes points: a scale of 1".into());
                }
                let x = input["x"].as_i64().unwrap_or(-1);
                let y = input["y"].as_i64().unwrap_or(-1);
                self.pointers.borrow_mut().push((x, y));
                if let Some(index) = self.element_at(x, y) {
                    self.touch(index);
                }
                Ok(Value::Null)
            }
            "station_type" => {
                let focus = self
                    .focus
                    .borrow()
                    .clone()
                    .ok_or("nothing has the focus to type into")?;
                let text = input["text"].as_str().unwrap_or_default();
                self.values
                    .borrow_mut()
                    .entry(focus)
                    .or_default()
                    .push_str(text);
                Ok(Value::Null)
            }
            "station_key" => {
                self.keys
                    .borrow_mut()
                    .push(input["combo"].as_str().unwrap_or_default().to_string());
                Ok(Value::Null)
            }
            other => Err(format!("the fake station has no tool {other}")),
        }
    }
}

/// A desktop target on `station`, opened as the step names `os`.
fn open<'a>(station: &'a FakeStation, os: &str) -> Result<Gui<'a>, String> {
    Gui::open(Box::new(station), "ws-1", os)
}

#[test]
fn a_station_tree_reads_as_its_screen_with_its_controls_and_their_roles() {
    let station = FakeStation::new(LINUX, shop(false));
    let mut gui = open(&station, LINUX).unwrap();
    let tree = gui.tree().unwrap();
    assert_eq!(tree.title, "Cart");
    let pay = tree.elements.iter().find(|e| e.label == "Pay").unwrap();
    assert_eq!(pay.role, "button");
    assert_eq!(pay.id, "n.0", "a control's id is its child-index path");
    assert!(
        tree.elements
            .iter()
            .any(|e| e.id == "n.1" && e.role == "textbox"),
        "a text field maps to a textbox"
    );
    assert_eq!(
        tree.actionable().count(),
        2,
        "the button and the text field"
    );
    assert!(tree.shows("pay"));
}

#[test]
fn clicks_types_keys_and_pictures_go_through_the_station_pointer_and_keys() {
    let station = FakeStation::new(LINUX, shop(false));
    let mut gui = open(&station, LINUX).unwrap();
    gui.tree().unwrap();
    gui.act(&Action::Type {
        element: "n.1".into(),
        text: "no ice".into(),
    })
    .unwrap();
    assert!(gui.tree().unwrap().shows("no ice"));
    assert_eq!(
        station.pointers.borrow()[0],
        (170, 90),
        "typing first clicks the field's middle"
    );
    gui.act(&Action::Press {
        key: "Enter".into(),
    })
    .unwrap();
    assert_eq!(*station.keys.borrow(), vec!["Enter".to_string()]);
    let cart = gui.screenshot().unwrap();
    assert_eq!(cart.mime, "image/png");
    assert!(cart.bytes.starts_with(&[0x89, b'P', b'N', b'G']), "a PNG");
    gui.act(&Action::Click {
        element: "n.0".into(),
    })
    .unwrap();
    assert_eq!(gui.tree().unwrap().title, "Receipt");
    assert!(station.pointers.borrow().contains(&(170, 40)));
    assert_ne!(gui.screenshot().unwrap().bytes, cart.bytes);
    assert!(gui.clock_ms().is_some());
}

#[test]
fn a_bad_element_or_a_click_into_a_button_is_refused() {
    let station = FakeStation::new(WINDOWS, shop(false));
    let mut gui = open(&station, WINDOWS).unwrap();
    gui.tree().unwrap();
    assert!(
        gui.act(&Action::Click {
            element: "n.9".into()
        })
        .unwrap_err()
        .contains("no element n.9")
    );
    assert!(
        gui.act(&Action::Type {
            element: "n.0".into(),
            text: "x".into()
        })
        .unwrap_err()
        .contains("not a textbox")
    );
}

#[test]
fn a_seeded_bug_fails_the_scenario_and_the_fixed_station_passes_on_linux_and_on_windows() {
    for os in [LINUX, WINDOWS] {
        let buggy = FakeStation::new(os, shop(true));
        let bug = run(
            pay_scenario(),
            press_pay,
            || -> Result<Box<dyn Target>, String> { Ok(Box::new(open(&buggy, os)?)) },
        );
        assert_eq!(bug.target, kind_of(os).unwrap());
        assert_eq!(
            bug.found.len(),
            1,
            "{os}: the seeded bug is a finding: {}",
            bug.summary()
        );
        assert!(matches!(bug.found[0].ruling, Ruling::Confirmed { .. }));
        let fixed = FakeStation::new(os, shop(false));
        let pass = run(
            pay_scenario(),
            press_pay,
            || -> Result<Box<dyn Target>, String> { Ok(Box::new(open(&fixed, os)?)) },
        );
        assert!(
            pass.found.is_empty(),
            "{os}: the fixed station passes: {}",
            pass.summary()
        );
    }
}

#[test]
fn a_station_of_the_other_os_offline_or_unknown_is_refused_and_a_queued_lease_is_released() {
    let linux = FakeStation::new(LINUX, shop(false));
    let why = open(&linux, WINDOWS).err().unwrap();
    assert!(why.contains("runs linux"), "{why}");
    let mut offline = FakeStation::new(LINUX, shop(false));
    offline.online = false;
    assert!(open(&offline, LINUX).err().unwrap().contains("not online"));
    let unknown = Gui::open(Box::new(&linux), "ws-9", LINUX).err().unwrap();
    assert!(unknown.contains("no station ws-9"), "{unknown}");
    let mut queued = FakeStation::new(LINUX, shop(false));
    queued.queued = true;
    let why = open(&queued, LINUX).err().unwrap();
    assert!(why.contains("no pixel session yet"), "{why}");
    assert_eq!(queued.released.get(), 1, "the queued lease is given back");
}

#[test]
fn the_lease_is_released_when_the_target_goes() {
    let station = FakeStation::new(LINUX, shop(false));
    {
        let _gui = open(&station, LINUX).unwrap();
        assert_eq!(station.released.get(), 0, "held while the target lives");
    }
    assert_eq!(station.released.get(), 1);
}

#[test]
fn a_step_names_one_target_and_a_desktop_names_linux_or_windows() {
    let base = UserTest {
        planner: "mock/script".into(),
        driver: "mock/script".into(),
        escalate: "mock/script".into(),
        checker: "mock/script".into(),
        stand_in: None,
        web: None,
        tui: None,
        desktop: None,
        acceptance: vec![],
        threshold: 0.6,
        actions: 8,
        exploratory: 0,
    };
    let linux = UserTest {
        desktop: Some(DesktopTarget {
            station: "ws-1".into(),
            os: "linux".into(),
        }),
        ..base.clone()
    };
    assert!(linux.check().is_ok());
    let macos = UserTest {
        desktop: Some(DesktopTarget {
            station: "ws-1".into(),
            os: "macos".into(),
        }),
        ..base.clone()
    };
    assert!(macos.check().unwrap_err().contains("linux or windows"));
    let both = UserTest {
        web: Some(WebTarget::default()),
        ..linux.clone()
    };
    assert!(both.check().unwrap_err().contains("one target"));
}
