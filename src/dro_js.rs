use boa_engine::{Context, JsResult, JsValue, NativeFunction, Source};
use std::cell::RefCell;

thread_local! { static LOG: RefCell<Vec<String>> = RefCell::new(Vec::new()); }

fn log(_: &JsValue, args: &[JsValue], c: &mut Context<'_>) -> JsResult<JsValue> {
    let s: Vec<String> = args.iter().map(|v| v.to_string(c).map(|s| s.to_std_string_escaped()).unwrap_or_default()).collect();
    LOG.with(|l| l.borrow_mut().push(s.join(" ")));
    Ok(JsValue::undefined())
}

pub struct JsRuntime {
    ctx: Context<'static>,
}

impl JsRuntime {
    pub fn new() -> Result<Self, String> {
        let mut ctx = Context::default();
        ctx.runtime_limits_mut().set_loop_iteration_limit(5_000_000); // stop infinite loops
        ctx.runtime_limits_mut().set_recursion_limit(400);
        ctx.register_global_builtin_callable("__log", 1, NativeFunction::from_fn_ptr(log)).map_err(|e| format!("{e:?}"))?;
        let boot = "var console={log:function(){__log(Array.prototype.join.call(arguments,' '))}};console.warn=console.error=console.info=console.log;\
                    var alert=function(m){__log('alert: '+m)};var window=this;var document={title:''};";
        ctx.eval(Source::from_bytes(boot)).map_err(|e| format!("{e:?}"))?;
        Ok(JsRuntime { ctx })
    }
    pub fn eval(&mut self, code: &str) -> Result<String, String> {
        let v = self.ctx.eval(Source::from_bytes(code)).map_err(|e| format!("JS error: {e:?}"))?;
        v.to_string(&mut self.ctx).map(|s| s.to_std_string_escaped()).map_err(|e| format!("{e:?}"))
    }
    pub fn take_logs(&self) -> Vec<String> {
        LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
    }
}
