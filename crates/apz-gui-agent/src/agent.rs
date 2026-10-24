//! The station tools answered on a desktop: a lease opens the desktop's one session, and the screen tools act on it.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use apz_desktop::{Desktop, DesktopError, SessionSpec};
use apz_station::{Button, ImageFormat, InputAction, Selector, SessionId, SessionKind, UiAction};
use base64::Engine as _;
use serde_json::{Value, json};

/// The operating system of this agent's desktop, as the hub's facts name it.
pub const OS: &str = "linux";
/// The size of the session a lease opens, in points.
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 800;

/// A desktop shared by the connections that drive it: one call at a time, as the daemon's request loop makes them.
pub type SharedDesk = Arc<Mutex<Box<dyn Desktop>>>;

/// The shared desk for `desk`.
pub fn share(desk: Box<dyn Desktop>) -> SharedDesk {
    Arc::new(Mutex::new(desk))
}

/// One connection's view of the desktop: its lease (one session at a time), and the screen tools on it.
pub struct Agent {
    desk: SharedDesk,
    session: Option<SessionId>,
}

impl Agent {
    pub fn new(desk: SharedDesk) -> Agent {
        Agent {
            desk,
            session: None,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Box<dyn Desktop>> {
        self.desk.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Answers one request: `hello`, the lease and its release, or a screen tool on the leased session.
    pub fn handle(&mut self, tool: &str, input: &Value) -> Result<Value, String> {
        match tool {
            "hello" => Ok(json!({"os": OS})),
            "station_lease" => self.lease(),
            "station_release" => self.release(),
            _ => {
                let session = self.session.ok_or("the desktop is not leased")?;
                self.screen(session, tool, input)
            }
        }
    }

    fn lease(&mut self) -> Result<Value, String> {
        if self.session.is_some() {
            return Err("the desktop is already leased".into());
        }
        let spec = SessionSpec {
            agent: 1,
            kind: SessionKind::Pixel,
            width: WIDTH,
            height: HEIGHT,
        };
        let session = self.lock().open_session(&spec).map_err(text)?;
        self.session = Some(session);
        Ok(json!({"id": "fork-lease", "session": session, "state": "active", "position": null}))
    }

    fn release(&mut self) -> Result<Value, String> {
        if let Some(session) = self.session.take() {
            self.lock().close_session(session).map_err(text)?;
        }
        Ok(Value::Null)
    }

    fn screen(&mut self, session: SessionId, tool: &str, input: &Value) -> Result<Value, String> {
        match tool {
            "station_ui_tree" => {
                let depth = u8::try_from(input["depth"].as_u64().unwrap_or(4))
                    .unwrap_or(8)
                    .clamp(1, 8);
                let node = self.lock().ui_tree(session, None, depth).map_err(text)?;
                serde_json::to_value(node).map_err(|why| why.to_string())
            }
            "station_screenshot" => {
                let frame = self.lock().capture(session).map_err(text)?;
                let png = apz_desktop::encode(&frame, ImageFormat::Png, 90).map_err(text)?;
                Ok(json!({
                    "w": frame.w,
                    "h": frame.h,
                    "format": "png",
                    "data_b64": base64::engine::general_purpose::STANDARD.encode(png),
                }))
            }
            "station_click" => {
                let action = InputAction::Click {
                    x: number(input, "x")?,
                    y: number(input, "y")?,
                    button: Button::Left,
                    count: 1,
                };
                self.input(session, &action)
            }
            "station_type" => {
                let action = InputAction::Type {
                    text: string(input, "text")?,
                };
                self.input(session, &action)
            }
            "station_key" => {
                let action = InputAction::Key {
                    combo: string(input, "combo")?,
                };
                self.input(session, &action)
            }
            "station_ui_act" => {
                let selector: Selector = serde_json::from_value(
                    input.get("selector").cloned().unwrap_or_else(|| json!({})),
                )
                .map_err(|why| why.to_string())?;
                let mut body = json!({"act": string(input, "act")?});
                if let Some(value) = input.get("value").and_then(Value::as_str) {
                    body["value"] = json!(value);
                }
                let action: UiAction =
                    serde_json::from_value(body).map_err(|why| why.to_string())?;
                self.lock()
                    .ui_act(session, &selector, &action)
                    .map_err(text)?;
                Ok(Value::Null)
            }
            other => Err(format!("{other} is not a desktop tool")),
        }
    }

    fn input(&mut self, session: SessionId, action: &InputAction) -> Result<Value, String> {
        self.lock().input(session, action).map_err(text)?;
        Ok(Value::Null)
    }
}

fn text(why: DesktopError) -> String {
    why.to_string()
}

fn number(input: &Value, key: &str) -> Result<i32, String> {
    input[key]
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| format!("`{key}` is a whole number of points"))
}

fn string(input: &Value, key: &str) -> Result<String, String> {
    input[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("`{key}` is a string"))
}

#[cfg(test)]
mod tests {
    use super::share;
    use super::*;
    use crate::script::{ScriptDesk, shop};

    #[test]
    fn a_lease_reads_the_tree_clicks_a_button_and_releases_the_desktop() {
        let mut agent = Agent::new(share(Box::new(ScriptDesk::new(shop(false)))));
        assert_eq!(agent.handle("hello", &Value::Null).unwrap()["os"], "linux");
        assert!(
            agent.handle("station_ui_tree", &Value::Null).is_err(),
            "no lease yet"
        );
        let lease = agent
            .handle("station_lease", &json!({"kind": "pixel"}))
            .unwrap();
        assert_eq!(lease["session"], 1);
        let tree = agent
            .handle("station_ui_tree", &json!({"depth": 2}))
            .unwrap();
        assert_eq!(tree["name"], "Cart");
        assert_eq!(tree["children"].as_array().unwrap().len(), 2);
        agent
            .handle("station_click", &json!({"x": 170, "y": 40, "scale": 1.0}))
            .unwrap();
        let tree = agent.handle("station_ui_tree", &json!({})).unwrap();
        assert_eq!(tree["name"], "Receipt", "the button led to the receipt");
        let shot = agent
            .handle("station_screenshot", &json!({"scale": 1.0}))
            .unwrap();
        assert_eq!(shot["format"], "png");
        assert!(shot["data_b64"].as_str().is_some_and(|b| !b.is_empty()));
        agent.handle("station_release", &Value::Null).unwrap();
        assert!(
            agent.handle("station_ui_tree", &json!({})).is_err(),
            "released"
        );
    }

    #[test]
    fn a_second_lease_is_refused_and_a_bad_point_names_its_key() {
        let mut agent = Agent::new(share(Box::new(ScriptDesk::new(shop(false)))));
        agent.handle("station_lease", &json!({})).unwrap();
        assert!(
            agent
                .handle("station_lease", &json!({}))
                .unwrap_err()
                .contains("already leased")
        );
        let why = agent
            .handle("station_click", &json!({"x": "left", "y": 1}))
            .unwrap_err();
        assert!(why.contains("`x`"), "{why}");
    }
}
