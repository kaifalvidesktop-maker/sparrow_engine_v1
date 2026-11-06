//! JS engine abstraction: DOM/bridge code should depend on `JsEngine`, never on boa directly.
//! Swap Boa -> QuickJS -> V8 by adding one `impl JsEngine` and one arm in `create()`.
use crate::dro_js::JsRuntime;

pub trait JsEngine {
    fn name(&self) -> &'static str;
    fn eval(&mut self, code: &str) -> Result<String, String>;
    fn take_logs(&self) -> Vec<String>;
}

impl JsEngine for JsRuntime {
    fn name(&self) -> &'static str { "boa" }
    fn eval(&mut self, code: &str) -> Result<String, String> { JsRuntime::eval(self, code) }
    fn take_logs(&self) -> Vec<String> { JsRuntime::take_logs(self) }
}

/// JavaScript disabled (settings: "JS off").
pub struct NullJs;
impl JsEngine for NullJs {
    fn name(&self) -> &'static str { "off" }
    fn eval(&mut self, _: &str) -> Result<String, String> { Err("JavaScript is disabled".into()) }
    fn take_logs(&self) -> Vec<String> { Vec::new() }
}

/// kind: "boa" (default) | "off". Add "quickjs" / "v8" here later.
pub fn create(kind: &str) -> Result<Box<dyn JsEngine>, String> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "" | "boa" => Ok(Box::new(JsRuntime::new()?)),
        "off" => Ok(Box::new(NullJs)),
        other => Err(format!("unknown JS engine: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn off_engine_refuses() { assert!(create("off").unwrap().eval("1").is_err()); }
    #[test] fn unknown_engine_errors() { assert!(create("nope").is_err()); }
    #[test] fn boa_evaluates() {
        let mut j = create("boa").unwrap();
        assert_eq!(j.eval("1+2").unwrap(), "3");
        assert_eq!(j.name(), "boa");
    }
}
