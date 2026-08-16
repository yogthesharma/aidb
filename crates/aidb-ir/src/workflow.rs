//! Declared workflow graph. Compiles to IR; not a LangGraph SDK.

use crate::{LlmContent, LogicalNode, LogicalOp, LogicalPlan, Schema};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Workflow {
    Then(Vec<Workflow>),
    Parallel(Vec<Workflow>),
    Branch {
        when: String,
        then: Box<Workflow>,
        else_: Option<Box<Workflow>>,
    },
    Loop {
        body: Box<Workflow>,
        until: String,
        max: u32,
    },
    Search {
        query: String,
        k: i64,
    },
    Generate {
        prompt: String,
        content: Option<String>,
    },
    Tool {
        name: String,
    },
    /// Runtime pause. Not an IR operator — `to_logical` drops these.
    Pause {
        status: String,
        message: String,
    },
    /// Agent-only: the model chooses the next operator. Workflows stay static.
    Decide {
        tools: Vec<String>,
        max: u32,
    },
}

impl Workflow {
    pub fn to_logical(&self) -> LogicalPlan {
        LogicalPlan {
            root: self.compile("w"),
        }
    }

    pub fn is_run_pause(&self) -> bool {
        matches!(self, Self::Pause { .. })
    }

    fn compile_steps(steps: &[Workflow], id: &str) -> Vec<LogicalNode> {
        steps
            .iter()
            .filter(|step| !step.is_run_pause())
            .enumerate()
            .map(|(i, step)| step.compile(&format!("{id}.{i}")))
            .collect()
    }

    fn compile(&self, id: &str) -> LogicalNode {
        match self {
            Self::Then(steps) => LogicalNode::new(
                id,
                LogicalOp::Then,
                Schema::default(),
                Schema::generated_text(),
                Self::compile_steps(steps, id),
            ),
            Self::Parallel(steps) => LogicalNode::new(
                id,
                LogicalOp::Parallel,
                Schema::default(),
                Schema::generated_text(),
                Self::compile_steps(steps, id),
            ),
            Self::Branch { when, then, else_ } => {
                let mut children = vec![then.compile(&format!("{id}.then"))];
                if let Some(else_step) = else_ {
                    children.push(else_step.compile(&format!("{id}.else")));
                }
                LogicalNode::new(
                    id,
                    LogicalOp::Branch {
                        predicate: when.clone(),
                    },
                    Schema::default(),
                    Schema::generated_text(),
                    children,
                )
            }
            Self::Loop { body, until, max } => LogicalNode::new(
                id,
                LogicalOp::Loop {
                    until: until.clone(),
                    max: (*max).max(1),
                },
                Schema::default(),
                Schema::generated_text(),
                vec![body.compile(&format!("{id}.body"))],
            ),
            Self::Search { query, k } => {
                let mut node = LogicalPlan::search(query, *k).root;
                node.id = id.to_string();
                node
            }
            Self::Generate { prompt, content } => {
                let content = match content {
                    Some(text) => LlmContent::Literal(text.clone()),
                    None => LlmContent::Column("context".into()),
                };
                LogicalNode::new(
                    id,
                    LogicalOp::Llm {
                        prompt: prompt.clone(),
                        content,
                    },
                    Schema::default(),
                    Schema::generated_text(),
                    Vec::new(),
                )
            }
            Self::Tool { name } => LogicalNode::new(
                id,
                LogicalOp::Tool { name: name.clone() },
                Schema::default(),
                Schema::generated_text(),
                Vec::new(),
            ),
            Self::Decide { tools, max } => LogicalNode::new(
                id,
                LogicalOp::Decide {
                    tools: tools.clone(),
                    max: (*max).max(1),
                },
                Schema::default(),
                Schema::generated_text(),
                Vec::new(),
            ),
            Self::Pause { .. } => LogicalNode::new(
                id,
                LogicalOp::Then,
                Schema::default(),
                Schema::generated_text(),
                Vec::new(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn then_search_generate_compiles() {
        let plan = Workflow::Then(vec![
            Workflow::Search {
                query: "refunds".into(),
                k: 5,
            },
            Workflow::Generate {
                prompt: "Summarize this".into(),
                content: None,
            },
        ])
        .to_logical();
        assert!(matches!(plan.root.op, LogicalOp::Then));
        assert_eq!(plan.root.id, "w");
        assert_eq!(plan.root.children[0].id, "w.0");
        assert!(matches!(plan.root.children[0].op, LogicalOp::TopK { k: 5 }));
        assert!(matches!(plan.root.children[1].op, LogicalOp::Llm { .. }));
        assert!(plan.is_workflow());
        assert!(!plan.is_search());
    }

    #[test]
    fn approve_is_not_an_ir_operator() {
        let plan = Workflow::Then(vec![
            Workflow::Search {
                query: "refunds".into(),
                k: 5,
            },
            Workflow::Pause {
                status: "awaiting_approval".into(),
                message: "send this?".into(),
            },
            Workflow::Generate {
                prompt: "Summarize this".into(),
                content: None,
            },
        ])
        .to_logical();
        assert_eq!(plan.root.children.len(), 2);
        assert!(matches!(plan.root.children[0].op, LogicalOp::TopK { k: 5 }));
        assert!(matches!(plan.root.children[1].op, LogicalOp::Llm { .. }));
    }

    #[test]
    fn decide_compiles_to_a_control_operator() {
        let plan = Workflow::Decide {
            tools: vec!["search".into(), "generate".into()],
            max: 4,
        }
        .to_logical();
        match &plan.root.op {
            LogicalOp::Decide { tools, max } => {
                assert_eq!(tools, &["search", "generate"]);
                assert_eq!(*max, 4);
            }
            other => panic!("{other:?}"),
        }
        assert!(plan.is_workflow());
    }
}
