//! Mark source functions whose loops can run while another task exists.
//! Calls and spawns connect functions within one possible task scope. A
//! component containing a spawn receives bounded loop checkpoints; isolated
//! components keep direct loops with no scheduler bookkeeping.

use std::collections::{HashMap, VecDeque};

use phoebe_syntax::syntax::{Body, CallArgument, Expr, ExprKind, Program, Statement};

pub(super) fn checkpoint_functions(program: &Program) -> Vec<bool> {
    let names = program
        .functions
        .iter()
        .enumerate()
        .map(|(id, function)| (function.name.as_str(), id))
        .collect::<HashMap<_, _>>();
    let mut graph = Scan {
        names,
        edges: vec![Vec::new(); program.functions.len()],
        spawn_sources: Vec::new(),
        current: 0,
    };
    for (id, function) in program.functions.iter().enumerate() {
        graph.current = id;
        graph.body(&function.body);
    }
    let mut checkpoint = vec![false; program.functions.len()];
    let mut ready = VecDeque::new();
    for source in graph.spawn_sources {
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
    checkpoint
}

struct Scan<'a> {
    names: HashMap<&'a str, usize>,
    edges: Vec<Vec<usize>>,
    spawn_sources: Vec<usize>,
    current: usize,
}

impl Scan<'_> {
    fn connect(&mut self, name: &str) {
        if let Some(&other) = self.names.get(name) {
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
            ExprKind::Copy(value) | ExprKind::Neg(value) | ExprKind::Try(value) => self.expr(value),
            ExprKind::Add(a, b)
            | ExprKind::Sub(a, b)
            | ExprKind::Mul(a, b)
            | ExprKind::Eq(a, b)
            | ExprKind::Lt(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::VariantCall(_, _, value) => self.expr(value),
            ExprKind::Call(name, args) => {
                self.connect(name);
                self.arguments(args);
            }
            ExprKind::QualifiedCall(_, _, args) => self.arguments(args),
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
            Statement::RequestUnpackVal { request, .. } => self.expr(request),
            Statement::With { value, body, .. } => {
                self.expr(value);
                self.body(body);
            }
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
