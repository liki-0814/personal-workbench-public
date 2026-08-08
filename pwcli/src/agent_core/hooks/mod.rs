/// 钩子事件类型
#[derive(Debug, Clone)]
pub enum HookEvent {
    ToolPreExecute {
        tool_name: String,
        arguments: String,
    },
    ToolPostExecute {
        tool_name: String,
        result: String,
        duration_ms: u64,
    },
    TurnStart {
        turn_number: u32,
    },
    TurnComplete {
        turn_number: u32,
        duration_ms: u64,
    },
}

/// 钩子执行结果
#[derive(Debug, Clone)]
pub struct HookRunResult {
    pub event: HookEvent,
    pub handled: bool,
}

type HookHandler = Box<dyn Fn(&HookEvent) + Send + Sync>;

/// 钩子 runner
#[derive(Default)]
pub struct HookRunner {
    handlers: Vec<HookHandler>,
}

impl HookRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_tool_execute<F>(&mut self, handler: F)
    where
        F: Fn(&HookEvent) + Send + Sync + 'static,
    {
        self.handlers.push(Box::new(handler));
    }

    pub fn run(&self, event: &HookEvent) -> HookRunResult {
        for handler in &self.handlers {
            handler(event);
        }
        HookRunResult {
            event: event.clone(),
            handled: !self.handlers.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_hook_runner() {
        let counter = AtomicUsize::new(0);
        let mut runner = HookRunner::new();
        runner.on_tool_execute(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });

        let event = HookEvent::ToolPreExecute {
            tool_name: "test".to_string(),
            arguments: "{}".to_string(),
        };
        let result = runner.run(&event);
        assert!(result.handled);
    }
}
