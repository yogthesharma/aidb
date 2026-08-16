//! Goal language frontend. Emits workflow IR. Not a second store.

use crate::{LlmContent, LogicalPlan, Workflow};

#[derive(Debug, Clone, PartialEq)]
pub struct GoalSpec {
    pub task: String,
    pub data: Vec<String>,
    pub goal: String,
    pub read_only: bool,
    pub max_usd: Option<f64>,
    pub max_ms: Option<u64>,
    pub k: i64,
    pub source: String,
}

impl GoalSpec {
    pub fn search_query(&self) -> String {
        let mut parts = vec![self.goal.clone(), self.task.clone()];
        parts.extend(self.data.iter().cloned());
        parts
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn prompt(&self) -> String {
        let data = if self.data.is_empty() {
            "available documents".into()
        } else {
            self.data.join(", ")
        };
        format!(
            "TASK {}. GOAL {}. DATA {}. Answer from retrieved data only. End with DONE.",
            self.task, self.goal, data
        )
    }

    pub fn to_workflow(&self) -> Workflow {
        let mut steps = vec![Workflow::Search {
            query: self.search_query(),
            k: self.k,
        }];
        if self.data.iter().any(|d| d == "documents") {
            steps.push(Workflow::Generate {
                prompt: self.prompt(),
                content: Some(String::new()),
            });
        } else {
            steps.push(Workflow::Generate {
                prompt: self.prompt(),
                content: None,
            });
        }
        Workflow::Then(steps)
    }

    pub fn to_logical(&self) -> LogicalPlan {
        if self.emits_generate_over_documents() {
            return LogicalPlan::generate_naive(
                self.prompt(),
                LlmContent::Column("content".into()),
                "documents",
                None,
            );
        }
        self.to_workflow().to_logical()
    }

    pub fn emits_generate_over_documents(&self) -> bool {
        !self.data.is_empty() && self.data.iter().all(|d| d == "documents")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LogicalOp;

    #[test]
    fn goal_emits_search_then_generate() {
        let spec = GoalSpec {
            task: "investigate_incident".into(),
            data: vec!["logs".into(), "deployments".into()],
            goal: "identify_root_cause".into(),
            read_only: true,
            max_usd: Some(1.0),
            max_ms: Some(300_000),
            k: 5,
            source: String::new(),
        };
        let plan = spec.to_logical();
        assert!(matches!(plan.root.op, LogicalOp::Then));
        assert!(matches!(plan.root.children[0].op, LogicalOp::TopK { k: 5 }));
        assert!(matches!(plan.root.children[1].op, LogicalOp::Llm { .. }));
    }

    #[test]
    fn documents_data_emits_generate_over_table() {
        let spec = GoalSpec {
            task: "summarize".into(),
            data: vec!["documents".into()],
            goal: "How do refunds work?".into(),
            read_only: true,
            max_usd: None,
            max_ms: None,
            k: 5,
            source: String::new(),
        };
        let plan = spec.to_logical();
        assert!(matches!(plan.root.op, LogicalOp::Llm { .. }));
        assert!(matches!(
            plan.root.children[0].op,
            LogicalOp::Scan { ref table } if table == "documents"
        ));
    }
}
