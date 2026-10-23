pub fn greet(name: &str) -> String {
    format!("hello, {name}")
}

pub fn welcome() -> String {
    greet("plaza")
}
