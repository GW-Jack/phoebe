//! Mark source functions whose loops can run while another task exists.
//! Calls and spawns connect functions within one possible task scope. A
//! component containing a spawn receives bounded loop checkpoints; isolated
//! components keep direct loops with no scheduler bookkeeping.

use std::collections::{HashMap, VecDeque};

use phoebe_syntax::syntax::{Body, CallArgument, Expr, ExprKind, Program, Statement};

pub(super) struct Analysis {
    pub checkpointed: Vec<bool>,
    pub effects: HashMap<String, bool>,
    pub call_effects: Vec<Vec<bool>>,
}

pub(super) fn analyze(program: &Program) -> Analysis {
    let graph = Scan::program(program);
    let mut checkpoint = vec![false; program.functions.len()];
    let mut ready = VecDeque::new();
    for &source in &graph.spawn_sources {
        if !checkpoint[source] {
            checkpoint[source] = true;
            ready.push_back(source);
        }
    }
    while let Some(function) = ready.pop_front() {
        for &neighbor in &graph.edges[function] {
            if !checkpoint[neighbor] {
                checkpoint[neighbor] = true;
                ready.push_back(neighbor);
            }
        }
    }
    let mut callers = vec![Vec::new(); program.functions.len()];
    for (caller, callees) in graph.calls.iter().enumerate() {
        for &callee in callees {
            callers[callee].push(caller);
        }
    }
    let mut effects = vec![false; program.functions.len()];
    let mut ready = VecDeque::new();
    for &source in &graph.spawn_sources {
        if !effects[source] {
            effects[source] = true;
            ready.push_back(source);
        }
    }
    while let Some(callee) = ready.pop_front() {
        for &caller in &callers[callee] {
            if !effects[caller] {
                effects[caller] = true;
                ready.push_back(caller);
            }
        }
    }
    let call_effects = graph
        .calls
        .iter()
        .map(|calls| calls.iter().map(|callee| effects[*callee]).collect())
        .collect();
    let effects = program
        .functions
        .iter()
        .zip(effects)
        .map(|(function, effect)| (function.name.clone(), effect))
        .collect();
    Analysis {
        checkpointed: checkpoint,
        effects,
        call_effects,
    }
}

#[cfg(test)]
fn checkpoint_functions(program: &Program) -> Vec<bool> {
    analyze(program).checkpointed
}

pub(super) fn body_can_spawn(body: &Body, effects: &HashMap<String, bool>) -> bool {
    fn arguments(args: &[CallArgument], effects: &HashMap<String, bool>) -> bool {
        args.iter().any(|arg| match arg {
            CallArgument::Value(value) => expr(value, effects),
            CallArgument::Borrow { .. } => false,
        })
    }
    fn expr(value: &Expr, effects: &HashMap<String, bool>) -> bool {
        match &value.kind {
            ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Name(_) | ExprKind::Member(_, _) => {
                false
            }
            ExprKind::Copy(value)
            | ExprKind::Neg(value)
            | ExprKind::Not(value)
            | ExprKind::Try(value) => expr(value, effects),
            ExprKind::Add(a, b)
            | ExprKind::Sub(a, b)
            | ExprKind::Mul(a, b)
            | ExprKind::Eq(a, b)
            | ExprKind::Ne(a, b)
            | ExprKind::Lt(a, b)
            | ExprKind::Le(a, b)
            | ExprKind::Gt(a, b)
            | ExprKind::Ge(a, b)
            | ExprKind::And(a, b)
            | ExprKind::Or(a, b) => expr(a, effects) || expr(b, effects),
            ExprKind::VariantCall(_, _, value) => expr(value, effects),
            ExprKind::Call(name, args) => {
                effects.get(name).copied().unwrap_or(true) || arguments(args, effects)
            }
            ExprKind::QualifiedCall(name, member, args) => {
                effects
                    .get(&format!("{name}.{member}"))
                    .copied()
                    .unwrap_or(false)
                    || arguments(args, effects)
            }
            ExprKind::If(condition, yes, no) => {
                expr(condition, effects)
                    || body_can_spawn(yes, effects)
                    || body_can_spawn(no, effects)
            }
            ExprKind::When(subject, arms) => {
                expr(subject, effects) || arms.iter().any(|arm| body_can_spawn(&arm.body, effects))
            }
            ExprKind::With { value, body, .. } => {
                expr(value, effects) || body_can_spawn(body, effects)
            }
        }
    }
    body.statements.iter().any(|statement| match statement {
        Statement::Spawn { .. } => true,
        Statement::Val(_, _, value)
        | Statement::Var(_, _, value)
        | Statement::Assign(_, _, value)
        | Statement::Expr(value) => expr(value, effects),
        Statement::ChannelVal { capacity, .. } | Statement::RequestChannelVal { capacity, .. } => {
            expr(capacity, effects)
        }
        Statement::UnpackVal { value, .. } => expr(value, effects),
        Statement::With { value, body, .. } => {
            expr(value, effects) || body_can_spawn(body, effects)
        }
        Statement::Scope { body, .. } => body_can_spawn(body, effects),
        Statement::While(condition, body) => {
            expr(condition, effects) || body_can_spawn(body, effects)
        }
        Statement::ReplyVal { .. } | Statement::Yield(_) => false,
    }) || body.tail.as_ref().is_some_and(|tail| expr(tail, effects))
}

impl<'a> Scan<'a> {
    fn program(program: &'a Program) -> Self {
        let names = program
            .functions
            .iter()
            .enumerate()
            .map(|(id, function)| (function.name.as_str(), id))
            .collect::<HashMap<_, _>>();
        let mut graph = Scan {
            names,
            edges: vec![Vec::new(); program.functions.len()],
            calls: vec![Vec::new(); program.functions.len()],
            spawn_sources: Vec::new(),
            current: 0,
        };
        for (id, function) in program.functions.iter().enumerate() {
            graph.current = id;
            graph.body(&function.body);
        }
        graph
    }
}

struct Scan<'a> {
    names: HashMap<&'a str, usize>,
    edges: Vec<Vec<usize>>,
    calls: Vec<Vec<usize>>,
    spawn_sources: Vec<usize>,
    current: usize,
}

impl Scan<'_> {
    fn connect(&mut self, name: &str) {
        if let Some(&other) = self.names.get(name) {
            self.calls[self.current].push(other);
            self.edges[self.current].push(other);
            self.edges[other].push(self.current);
        }
    }

    fn body(&mut self, body: &Body) {
        for statement in &body.statements {
            self.statement(statement);
        }
        if let Some(tail) = &body.tail {
            self.expr(tail);
        }
    }

    fn arguments(&mut self, args: &[CallArgument]) {
        for arg in args {
            if let CallArgument::Value(value) = arg {
                self.expr(value);
            }
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Name(_) | ExprKind::Member(_, _) => {}
            ExprKind::Copy(value)
            | ExprKind::Neg(value)
            | ExprKind::Not(value)
            | ExprKind::Try(value) => self.expr(value),
            ExprKind::Add(a, b)
            | ExprKind::Sub(a, b)
            | ExprKind::Mul(a, b)
            | ExprKind::Eq(a, b)
            | ExprKind::Ne(a, b)
            | ExprKind::Lt(a, b)
            | ExprKind::Le(a, b)
            | ExprKind::Gt(a, b)
            | ExprKind::Ge(a, b)
            | ExprKind::And(a, b)
            | ExprKind::Or(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::VariantCall(_, _, value) => self.expr(value),
            ExprKind::Call(name, args) => {
                self.connect(name);
                self.arguments(args);
            }
            ExprKind::QualifiedCall(name, member, args) => {
                self.connect(&format!("{name}.{member}"));
                self.arguments(args);
            }
            ExprKind::If(condition, yes, no) => {
                self.expr(condition);
                self.body(yes);
                self.body(no);
            }
            ExprKind::When(subject, arms) => {
                self.expr(subject);
                for arm in arms {
                    self.body(&arm.body);
                }
            }
            ExprKind::With { value, body, .. } => {
                self.expr(value);
                self.body(body);
            }
        }
    }

    fn statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Spawn { child, args, .. } => {
                self.spawn_sources.push(self.current);
                self.connect(child);
                self.arguments(args);
            }
            Statement::Val(_, _, value)
            | Statement::Var(_, _, value)
            | Statement::Assign(_, _, value)
            | Statement::Expr(value) => self.expr(value),
            Statement::ChannelVal { capacity, .. }
            | Statement::RequestChannelVal { capacity, .. } => self.expr(capacity),
            Statement::UnpackVal { value, .. } => self.expr(value),
            Statement::With { value, body, .. } => {
                self.expr(value);
                self.body(body);
            }
            Statement::Scope { body, .. } => self.body(body),
            Statement::While(condition, body) => {
                self.expr(condition);
                self.body(body);
            }
            Statement::ReplyVal { .. } | Statement::Yield(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_nested_spawn_and_leaves_disconnected_loop_direct() {
        let source = r#"
fn worker() -> I64 = 0
fn launch() -> Result<I64, ResourceError> {
  val value = if (true) {
    spawn worker()?;
    1
  } else { 0 };
  Result<I64, ResourceError>.Ok(value)
}
fn unrelated() -> I64 {
  var count = 0;
  while (count < 3) { count = count + 1; }
  count
}
fn main() -> I64 = unrelated()
"#;
        let program = phoebe_syntax::syntax::parse(source).unwrap();
        assert_eq!(checkpoint_functions(&program), [true, true, false, false]);
        let connected = source.replace(
            "fn main() -> I64 = unrelated()",
            "fn main() -> I64 { val ignored = launch(); unrelated() }",
        );
        let program = phoebe_syntax::syntax::parse(&connected).unwrap();
        assert_eq!(checkpoint_functions(&program), [true, true, true, true]);
    }
}
