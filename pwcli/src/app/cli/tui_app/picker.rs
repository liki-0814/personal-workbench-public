use std::collections::HashMap;

const COMMANDS: &[(&str, &str)] = &[
    ("/diff", "按需查看当前工作区 diff"),
    ("/new", "在 daemon 新建会话"),
    ("/sessions", "列出 daemon 会话"),
    ("/resume", "切换到 daemon 会话 ID"),
    ("/status", "查看 daemon、任务和 ACP 状态"),
    ("/permissions", "查看或切换权限模式"),
    ("/attention", "查看待处理审批和审阅"),
    ("/notifications", "查看后台完成通知"),
    ("/resolve", "处理 attention：ID version action"),
    ("/acp-permissions", "查看 ACP 原生权限请求"),
    ("/acp-resolve", "处理 ACP 权限：ID option"),
    ("/quit", "退出"),
    ("/exit", "退出"),
];

#[derive(Debug, Clone, Default)]
pub struct MenuSources {
    pub providers: Vec<String>,
    pub models_by_provider: HashMap<String, Vec<String>>,
    pub active_provider: String,
    pub sessions: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub(super) struct MenuItem {
    pub(super) label: String,
    pub(super) description: String,
    pub(super) replacement: String,
}

#[derive(Debug, Default)]
pub(super) struct MenuState {
    pub(super) items: Vec<MenuItem>,
    pub(super) selected: usize,
    pub(super) title: String,
}

impl MenuState {
    pub(super) fn active(&self) -> bool {
        !self.items.is_empty()
    }

    pub(super) fn refresh(&mut self, input: &str, sources: &MenuSources) {
        let previous_index = self.selected;
        let previous_label = self
            .items
            .get(self.selected)
            .map(|item| item.label.clone())
            .unwrap_or_default();

        if !input.starts_with('/') {
            self.clear();
            return;
        }

        if !input.contains(' ') {
            let query = input.to_lowercase();
            self.items = COMMANDS
                .iter()
                .filter(|(command, _)| command.starts_with(&query))
                .map(|(command, description)| MenuItem {
                    label: (*command).to_string(),
                    description: (*description).to_string(),
                    replacement: (*command).to_string(),
                })
                .collect();
            self.title = "命令".to_string();
        } else {
            let (command, query) = input.split_once(' ').unwrap_or((input, ""));
            let query = query.to_lowercase();
            self.items = match command {
                "/provider" => {
                    self.title = "Provider".to_string();
                    sources
                        .providers
                        .iter()
                        .filter(|provider| provider.to_lowercase().starts_with(&query))
                        .map(|provider| MenuItem {
                            label: provider.clone(),
                            description: String::new(),
                            replacement: format!("/provider {provider}"),
                        })
                        .collect()
                }
                "/model" => {
                    self.title = format!("Model · {}", sources.active_provider);
                    sources
                        .models_by_provider
                        .get(&sources.active_provider)
                        .map(|models| {
                            models
                                .iter()
                                .filter(|model| model.to_lowercase().starts_with(&query))
                                .map(|model| MenuItem {
                                    label: model.clone(),
                                    description: String::new(),
                                    replacement: format!("/model {model}"),
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                }
                "/resume" => {
                    self.title = "已保存会话".to_string();
                    sources
                        .sessions
                        .iter()
                        .filter(|(id, name)| {
                            id.to_lowercase().starts_with(&query)
                                || name.to_lowercase().starts_with(&query)
                        })
                        .map(|(id, name)| MenuItem {
                            label: name.clone(),
                            description: id.clone(),
                            replacement: format!("/resume {id}"),
                        })
                        .collect()
                }
                "/permissions" => {
                    self.title = "权限模式".to_string();
                    [
                        ("prompt", "请求批准"),
                        ("risk", "替我审批（推荐）"),
                        ("full", "完全访问权限"),
                    ]
                    .into_iter()
                    .filter(|(mode, _)| mode.starts_with(&query))
                    .map(|(mode, description)| MenuItem {
                        label: mode.to_string(),
                        description: description.to_string(),
                        replacement: format!("/permissions {mode}"),
                    })
                    .collect()
                }
                _ => {
                    self.title.clear();
                    Vec::new()
                }
            };
        }

        self.selected = self
            .items
            .iter()
            .position(|item| item.label == previous_label)
            .unwrap_or_else(|| previous_index.min(self.items.len().saturating_sub(1)));
        if self.items.is_empty() {
            self.selected = 0;
        }
    }

    pub(super) fn clear(&mut self) {
        self.items.clear();
        self.selected = 0;
        self.title.clear();
    }

    pub(super) fn move_up(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.items.len() - 1
        } else {
            self.selected - 1
        };
    }

    pub(super) fn move_down(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    pub(super) fn current(&self) -> Option<&MenuItem> {
        self.items.get(self.selected)
    }
}
