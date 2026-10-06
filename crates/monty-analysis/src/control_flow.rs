//! Bounded structured projection of upstream reachability. These nodes describe
//! Python control flow; hosts attach their own semantics to ordinary statements.
use crate::{Span, inspect_function_at};
use ruff_python_ast::{Expr, Stmt};
use ruff_text_size::Ranged;
use std::collections::BTreeSet;
use ty_python_semantic::types::export;

#[derive(Debug, Clone)]
pub struct FunctionFlow {
    pub statements: Vec<Stmt>,
    pub exit: FlowExit,
}
#[derive(Debug, Clone)]
pub enum FlowExit {
    Return(Option<Expr>),
    Branch {
        /// Present when this branch is the projection of one return expression.
        /// Consumers may lower that whole expression without reassembling flow.
        source_expression: Option<Expr>,
        test: Expr,
        selected: Option<bool>,
        yes: Box<FunctionFlow>,
        no: Box<FunctionFlow>,
    },
}
impl FunctionFlow {
    pub fn returns_only_none(&self) -> bool {
        match &self.exit {
            FlowExit::Return(value) => value.as_ref().is_none_or(|v| matches!(v, Expr::NoneLiteral(_))),
            FlowExit::Branch {
                selected: Some(true),
                yes,
                ..
            } => yes.returns_only_none(),
            FlowExit::Branch {
                selected: Some(false),
                no,
                ..
            } => no.returns_only_none(),
            FlowExit::Branch { yes, no, .. } => yes.returns_only_none() && no.returns_only_none(),
        }
    }
    pub fn expression(value: Expr) -> Self {
        let exit = match value {
            Expr::If(choice) => FlowExit::Branch {
                source_expression: Some(Expr::If(choice.clone())),
                test: *choice.test,
                selected: None,
                yes: Box::new(Self::expression(*choice.body)),
                no: Box::new(Self::expression(*choice.orelse)),
            },
            value => FlowExit::Return(Some(value)),
        };
        Self {
            statements: vec![],
            exit,
        }
    }
}

pub fn function_flow(source: &str, target: Span) -> Result<FunctionFlow, crate::AnalysisError> {
    inspect_function_at(source, Some(target), |db, file, function| {
        let (ranges, _) = export::reachable_statements(db, file, function)
            .map_err(|source| crate::AnalysisError::ControlFlowExportLimitExceeded { source })?;
        let reachable = ranges.into_iter().collect();
        sequence(&function.body, &reachable, &mut 4096, 0, &|expr| {
            export::condition_truth(db, file, expr)
        })
    })
}
fn sequence(
    statements: &[Stmt],
    reachable: &BTreeSet<(u32, u32)>,
    budget: &mut usize,
    depth: usize,
    truth: &impl Fn(&Expr) -> Option<bool>,
) -> Result<FunctionFlow, crate::AnalysisError> {
    if depth >= 128 || *budget == 0 {
        return Err(crate::AnalysisError::ControlFlowProjectionBudgetExceeded);
    }
    *budget -= 1;
    let mut steps = vec![];
    for (index, stmt) in statements.iter().enumerate() {
        if *budget == 0 {
            return Err(crate::AnalysisError::ControlFlowProjectionBudgetExceeded);
        }
        *budget -= 1;
        if !reachable.contains(&(stmt.start().to_u32(), stmt.end().to_u32())) {
            continue;
        }
        let exit = match stmt {
            Stmt::Return(ret) => Some(match ret.value.as_deref() {
                Some(value) => FunctionFlow::expression(value.clone()).exit,
                None => FlowExit::Return(None),
            }),
            Stmt::If(branch) => {
                let tail = &statements[index + 1..];
                let mut yes = branch.body.to_vec();
                yes.extend_from_slice(tail);
                let mut no = vec![];
                if let Some((first, rest)) = branch.elif_else_clauses.split_first() {
                    if let Some(test) = &first.test {
                        let mut next = branch.clone();
                        next.test = Box::new(test.clone());
                        next.body = first.body.clone();
                        next.elif_else_clauses = rest.to_vec();
                        no.push(Stmt::If(next));
                    } else {
                        no.extend_from_slice(&first.body);
                    }
                }
                no.extend_from_slice(tail);
                Some(FlowExit::Branch {
                    source_expression: None,
                    test: *branch.test.clone(),
                    selected: truth(&branch.test),
                    yes: Box::new(sequence(&yes, reachable, budget, depth + 1, truth)?),
                    no: Box::new(sequence(&no, reachable, budget, depth + 1, truth)?),
                })
            }
            _ => {
                steps.push(stmt.clone());
                None
            }
        };
        if let Some(exit) = exit {
            return Ok(FunctionFlow {
                statements: steps,
                exit,
            });
        }
    }
    // Python functions fall through to None; it is not a host "missing return".
    Ok(FunctionFlow {
        statements: steps,
        exit: FlowExit::Return(None),
    })
}
