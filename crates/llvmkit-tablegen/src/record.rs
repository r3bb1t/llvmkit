//! The TableGen record and value model, and the evaluation engine that
//! builds records from parsed statements.
//!
//! Ports `llvm/lib/TableGen/Record.cpp`.

use crate::*;

#[derive(Debug, Clone)]
pub(crate) enum Value {
    Unset,
    Int(i64),
    Bool(bool),
    String(String),
    List(Vec<Value>),
    Record(Rc<RecordValue>),
    SelfRef(String, String),
}

#[derive(Debug, Clone)]
pub(crate) struct RecordValue {
    pub(crate) name: Option<String>,
    pub(crate) classes: BTreeSet<String>,
    pub(crate) fields: BTreeMap<String, Value>,
    pub(crate) id: usize,
}

#[derive(Debug, Default)]
pub(crate) struct RecordBuilder {
    pub(crate) name: Option<String>,
    pub(crate) classes: BTreeSet<String>,
    pub(crate) fields: BTreeMap<String, Value>,
}

impl RecordBuilder {
    pub(crate) fn merge(&mut self, other: Rc<RecordValue>) {
        self.classes.extend(other.classes.iter().cloned());
        for (name, value) in &other.fields {
            self.fields.insert(name.clone(), value.clone());
        }
    }

    pub(crate) fn finish(self, id: usize) -> Rc<RecordValue> {
        Rc::new(RecordValue {
            name: self.name,
            classes: self.classes,
            fields: self.fields,
            id,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Scope {
    pub(crate) values: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EnvStack {
    pub(crate) scopes: Vec<Scope>,
}

impl EnvStack {
    pub(crate) fn new() -> Self {
        Self {
            scopes: vec![Scope::default()],
        }
    }

    pub(crate) fn push(&mut self) {
        self.scopes.push(Scope::default());
    }

    pub(crate) fn pop(&mut self) {
        self.scopes.pop();
    }

    pub(crate) fn set<N>(&mut self, name: N, value: Value)
    where
        N: Into<String>,
    {
        self.scopes
            .last_mut()
            .expect("scope stack has root")
            .values
            .insert(name.into(), value);
    }

    pub(crate) fn get(&self, name: &str) -> Option<Value> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.values.get(name).cloned())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LetOverride {
    pub(crate) name: String,
    pub(crate) value: Expr,
}

pub(crate) struct Engine {
    pub(crate) llvm_root: PathBuf,
    pub(crate) loaded: HashSet<String>,
    pub(crate) classes: HashMap<String, TemplateDef>,
    pub(crate) multiclasses: HashMap<String, TemplateDef>,
    pub(crate) records: BTreeMap<String, Rc<RecordValue>>,
    pub(crate) record_order: Vec<Rc<RecordValue>>,
    pub(crate) globals: EnvStack,
    pub(crate) let_stack: Vec<LetOverride>,
    pub(crate) next_record_id: Cell<usize>,
}

impl Engine {
    pub(crate) fn new(llvm_root: PathBuf) -> Self {
        Self {
            llvm_root,
            loaded: HashSet::new(),
            classes: HashMap::new(),
            multiclasses: HashMap::new(),
            records: BTreeMap::new(),
            record_order: Vec::new(),
            globals: EnvStack::new(),
            let_stack: Vec::new(),
            next_record_id: Cell::new(1),
        }
    }

    pub(crate) fn load_include(&mut self, include: &str) -> GenResult<()> {
        let mut env = self.globals.clone();
        self.load_include_into(include, &mut env)?;
        self.globals = env;
        Ok(())
    }

    pub(crate) fn load_include_into(&mut self, include: &str, env: &mut EnvStack) -> GenResult<()> {
        if !self.loaded.insert(include.to_owned()) {
            return Ok(());
        }
        let path = self.resolve_include(include);
        let text = fs::read_to_string(&path).map_err(|source| {
            TableGenError::new(format!("failed to read {}: {source}", path.display()))
        })?;
        let tokens = Lexer::new(include, &text).tokenize()?;
        let stmts = Parser::new(tokens).parse()?;
        for stmt in &stmts {
            self.execute_stmt(stmt, env, None)?;
        }
        Ok(())
    }

    pub(crate) fn resolve_include(&self, include: &str) -> PathBuf {
        let trimmed = include.strip_prefix("llvm/").unwrap_or(include);
        self.llvm_root.join("include").join("llvm").join(trimmed)
    }

    pub(crate) fn append_active_let_fields(&self, def: &mut TemplateDef, loc: &Located<Stmt>) {
        for let_override in &self.let_stack {
            def.body.push(Located {
                node: Stmt::Field(let_override.name.clone(), let_override.value.clone()),
                file: loc.file.clone(),
                line: loc.line,
                col: loc.col,
            });
        }
    }

    pub(crate) fn eval_assert(
        &mut self,
        cond: &Expr,
        msg: &Expr,
        env: &mut EnvStack,
        loc: &Located<Stmt>,
    ) -> GenResult<()> {
        if assert_references_skipped_field(cond) || assert_references_skipped_field(msg) {
            return Ok(());
        }
        if self.eval_bool(cond, env)? {
            Ok(())
        } else {
            let message = self.eval_to_string(msg, env)?;
            Err(loc.error(format!("assertion failed: {message}")))
        }
    }

    pub(crate) fn execute_stmt(
        &mut self,
        stmt: &Located<Stmt>,
        env: &mut EnvStack,
        multi_prefix: Option<&str>,
    ) -> GenResult<()> {
        match &stmt.node {
            Stmt::Include(path) => self.load_include_into(path, env),
            Stmt::Class(def) => {
                let mut def = def.clone();
                self.append_active_let_fields(&mut def, stmt);
                self.classes.insert(def.name.clone(), def);
                Ok(())
            }
            Stmt::MultiClass(def) => {
                let mut def = def.clone();
                self.append_active_let_fields(&mut def, stmt);
                self.multiclasses.insert(def.name.clone(), def);
                Ok(())
            }
            Stmt::Def(def) => self.execute_def(stmt, def, env, multi_prefix).map(|_| ()),
            Stmt::Defm(defm) => self.execute_defm(stmt, defm, env, multi_prefix),
            Stmt::Defvar(name, expr) => {
                let value = self.eval(expr, env)?;
                env.set(name.clone(), value);
                Ok(())
            }
            Stmt::Defset(name, type_name, body) => {
                let before = self.record_order.len();
                for child in body {
                    self.execute_stmt(child, env, multi_prefix)?;
                }
                let list = self.record_order[before..]
                    .iter()
                    .filter(|record| record.classes.contains(type_name))
                    .cloned()
                    .map(Value::Record)
                    .collect::<Vec<_>>();
                env.set(name.clone(), Value::List(list));
                Ok(())
            }
            Stmt::LetBlock(assigns, body) => {
                let old_len = self.let_stack.len();
                self.let_stack
                    .extend(assigns.iter().map(|(name, value)| LetOverride {
                        name: name.clone(),
                        value: value.clone(),
                    }));
                for child in body {
                    self.execute_stmt(child, env, multi_prefix)?;
                }
                self.let_stack.truncate(old_len);
                Ok(())
            }
            Stmt::Foreach(var, expr, body) => {
                let values = self.eval_to_list(expr, env)?;
                for value in values {
                    env.push();
                    env.set(var.clone(), value);
                    for child in body {
                        self.execute_stmt(child, env, multi_prefix)?;
                    }
                    env.pop();
                }
                Ok(())
            }
            Stmt::If(cond, then_body, else_body) => {
                let body = if self.eval_bool(cond, env)? {
                    then_body
                } else {
                    else_body
                };
                for child in body {
                    self.execute_stmt(child, env, multi_prefix)?;
                }
                Ok(())
            }
            Stmt::Field(_, _) => Ok(()),
            Stmt::Assert(cond, msg) => self.eval_assert(cond, msg, env, stmt),
        }
    }

    pub(crate) fn execute_def(
        &mut self,
        stmt: &Located<Stmt>,
        def: &DefStmt,
        env: &mut EnvStack,
        multi_prefix: Option<&str>,
    ) -> GenResult<Rc<RecordValue>> {
        let raw_name = self.eval_name(&def.name, env)?;
        let name = if let Some(prefix) = multi_prefix {
            if def.name.contains_ident("NAME") {
                raw_name
            } else {
                format!("{prefix}{raw_name}")
            }
        } else {
            raw_name
        };

        if self.records.contains_key(&name) {
            return Err(stmt.error(format!("duplicate def `{name}`")));
        }

        env.push();
        env.set("NAME", Value::String(name.clone()));
        let mut builder = RecordBuilder {
            name: Some(name.clone()),
            ..RecordBuilder::default()
        };
        for parent in &def.parents {
            let record = self.instantiate_class(parent, Some(&name), env)?;
            builder.merge(record);
        }
        self.apply_body_fields(&def.body, env, &mut builder)?;
        self.apply_let_overrides(env, &mut builder)?;
        env.pop();

        let record = builder.finish(self.alloc_record_id());
        self.records.insert(name.clone(), record.clone());
        self.record_order.push(record.clone());
        env.set(name, Value::Record(record.clone()));
        Ok(record)
    }

    pub(crate) fn execute_defm(
        &mut self,
        stmt: &Located<Stmt>,
        defm: &DefmStmt,
        env: &mut EnvStack,
        multi_prefix: Option<&str>,
    ) -> GenResult<()> {
        let raw_name = self.eval_name(&defm.name, env)?;
        let prefix = if let Some(parent_prefix) = multi_prefix {
            if defm.name.contains_ident("NAME") {
                raw_name
            } else {
                format!("{parent_prefix}{raw_name}")
            }
        } else {
            raw_name
        };
        for parent in &defm.parents {
            if self.multiclasses.contains_key(&parent.name) {
                self.expand_multiclass(stmt, parent, &prefix, env)?;
            } else if self.classes.contains_key(&parent.name) {
                // Defm class parents in this intrinsic closure are marker classes
                // used by upstream backends; they do not affect the IR intrinsic
                // metadata emitted here.
                continue;
            } else {
                return Err(stmt.error(format!("unknown multiclass `{}`", parent.name)));
            }
        }
        Ok(())
    }

    pub(crate) fn expand_multiclass(
        &mut self,
        stmt: &Located<Stmt>,
        app: &Apply,
        prefix: &str,
        env: &mut EnvStack,
    ) -> GenResult<()> {
        let def = self
            .multiclasses
            .get(&app.name)
            .cloned()
            .ok_or_else(|| stmt.error(format!("unknown multiclass `{}`", app.name)))?;
        env.push();
        env.set("NAME", Value::String(prefix.to_owned()));
        self.bind_template_args(&def.params, &app.args, env, Some(prefix))?;
        for parent in &def.parents {
            self.expand_multiclass(stmt, parent, prefix, env)?;
        }
        for child in &def.body {
            self.execute_stmt(child, env, Some(prefix))?;
        }
        env.pop();
        Ok(())
    }

    pub(crate) fn instantiate_class(
        &mut self,
        app: &Apply,
        record_name: Option<&str>,
        env: &mut EnvStack,
    ) -> GenResult<Rc<RecordValue>> {
        let def = self
            .classes
            .get(&app.name)
            .cloned()
            .ok_or_else(|| TableGenError::new(format!("unknown class `{}`", app.name)))?;
        env.push();
        let name_value = record_name.unwrap_or(&app.name).to_owned();
        env.set("NAME", Value::String(name_value.clone()));
        self.bind_template_args(&def.params, &app.args, env, record_name)?;
        let mut builder = RecordBuilder::default();
        builder.classes.insert(def.name.clone());
        for param in &def.params {
            if let Some(value) = env.get(&param.name) {
                builder.fields.insert(param.name.clone(), value);
            }
        }
        for parent in &def.parents {
            let record = self.instantiate_class(parent, record_name, env)?;
            builder.merge(record);
        }
        for (field, value) in &builder.fields {
            env.set(field.clone(), value.clone());
        }
        self.apply_body_fields(&def.body, env, &mut builder)?;
        env.pop();
        Ok(builder.finish(0))
    }

    pub(crate) fn bind_template_args(
        &mut self,
        params: &[TemplateParam],
        args: &[TemplateArg],
        env: &mut EnvStack,
        record_name: Option<&str>,
    ) -> GenResult<()> {
        let mut positional = Vec::new();
        let mut named = HashMap::new();
        for arg in args {
            match arg {
                TemplateArg::Pos(expr) => positional.push(expr),
                TemplateArg::Named(name, expr) => {
                    named.insert(name.as_str(), expr);
                }
            }
        }
        for (idx, param) in params.iter().enumerate() {
            let value = if let Some(expr) = positional.get(idx) {
                self.eval(expr, env)?
            } else if let Some(expr) = named.get(param.name.as_str()) {
                self.eval(expr, env)?
            } else if let Some(default) = &param.default {
                self.eval(default, env)?
            } else {
                Value::Unset
            };
            env.set(param.name.clone(), value);
        }
        if let Some(name) = record_name {
            env.set("NAME", Value::String(name.to_owned()));
        }
        Ok(())
    }

    pub(crate) fn apply_body_fields(
        &mut self,
        body: &[Located<Stmt>],
        env: &mut EnvStack,
        builder: &mut RecordBuilder,
    ) -> GenResult<()> {
        for stmt in body {
            match &stmt.node {
                Stmt::Field(name, expr) => {
                    if name == "TypeInfo" {
                        let value = self.synthesize_type_info(env)?;
                        builder.fields.insert(name.clone(), value.clone());
                        env.set(name.clone(), value);
                        continue;
                    }
                    if should_skip_field_eval(name) {
                        builder.fields.insert(name.clone(), Value::Unset);
                        env.set(name.clone(), Value::Unset);
                        continue;
                    }
                    let value = self.eval(expr, env).map_err(|err| {
                        TableGenError::new(format!(
                            "{} while evaluating field `{}` on `{}`",
                            err.message,
                            name,
                            builder.name.as_deref().unwrap_or("<anonymous>")
                        ))
                    })?;
                    builder.fields.insert(name.clone(), value.clone());
                    env.set(name.clone(), value);
                }
                Stmt::Assert(cond, msg) => self.eval_assert(cond, msg, env, stmt)?,
                other => {
                    self.execute_stmt(
                        &Located {
                            node: other.clone(),
                            file: stmt.file.clone(),
                            line: stmt.line,
                            col: stmt.col,
                        },
                        env,
                        None,
                    )?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn apply_let_overrides(
        &mut self,
        env: &mut EnvStack,
        builder: &mut RecordBuilder,
    ) -> GenResult<()> {
        for let_override in self.let_stack.clone() {
            let value = self.eval(&let_override.value, env)?;
            builder
                .fields
                .insert(let_override.name.clone(), value.clone());
            env.set(let_override.name, value);
        }
        Ok(())
    }

    pub(crate) fn synthesize_type_info(&self, env: &mut EnvStack) -> GenResult<Value> {
        let ret_types = match env.get("RetTypes").ok_or_else(|| {
            TableGenError::new("TypeInfo synthesis requires RetTypes to be evaluated first")
        })? {
            Value::List(items) => items,
            other => {
                return Err(TableGenError::new(format!(
                    "TypeInfo synthesis expected RetTypes list, got {other:?}"
                )));
            }
        };
        let param_types = match env.get("ParamTypes").ok_or_else(|| {
            TableGenError::new("TypeInfo synthesis requires ParamTypes to be evaluated first")
        })? {
            Value::List(items) => items,
            other => {
                return Err(TableGenError::new(format!(
                    "TypeInfo synthesis expected ParamTypes list, got {other:?}"
                )));
            }
        };
        let all_values = ret_types
            .iter()
            .cloned()
            .chain(param_types.iter().cloned())
            .collect::<Vec<_>>();
        let all_types = all_values
            .iter()
            .map(type_from_value)
            .collect::<GenResult<Vec<_>>>()?;
        let is_overloaded = all_types.iter().any(IntrType::is_any);
        let type_sig = compute_type_signature(&ret_types, &param_types)?;

        let mut builder = RecordBuilder::default();
        builder.classes.insert("TypeInfoGen".to_owned());
        builder
            .fields
            .insert("RetTypes".to_owned(), Value::List(ret_types));
        builder
            .fields
            .insert("ParamTypes".to_owned(), Value::List(param_types));
        builder
            .fields
            .insert("AllTypes".to_owned(), Value::List(all_values.clone()));
        builder
            .fields
            .insert("Types".to_owned(), Value::List(all_values));
        builder
            .fields
            .insert("isOverloaded".to_owned(), Value::Bool(is_overloaded));
        builder.fields.insert(
            "TypeSig".to_owned(),
            Value::List(
                type_sig
                    .into_iter()
                    .map(|value| Value::Int(value as i64))
                    .collect(),
            ),
        );
        Ok(Value::Record(builder.finish(self.alloc_record_id())))
    }

    pub(crate) fn eval_synthetic_field(
        &mut self,
        base: &Expr,
        field: &str,
        env: &mut EnvStack,
    ) -> GenResult<Option<Value>> {
        let Expr::Apply(app) = base else {
            return Ok(None);
        };
        if app.name != "ResolveArgCode" || field != "ret" {
            return Ok(None);
        }
        let args = positional_args(app)?;
        if args.len() != 4 {
            return Err(TableGenError::new("ResolveArgCode requires four arguments"));
        }

        let ax = self.eval_int(args[3], env).map_err(|err| {
            TableGenError::new(format!(
                "{} while resolving ResolveArgCode encoded operand",
                err.message
            ))
        })?;
        let ah = ax & 0xFF00;
        let al = ax & 0x00FF;
        let ret = match ah {
            0x100 => {
                let ac_idx = self.eval_int(args[2], env).map_err(|err| {
                    TableGenError::new(format!(
                        "{} while resolving EncAnyType ACIdx for ax {ax:#x}",
                        err.message
                    ))
                })?;
                (ac_idx << 3) | al
            }
            0x200 => {
                let mapping = self.eval_int_list(args[0], env)?;
                let num = list_int_at(&mapping, al, "ResolveArgCode Mapping")?;
                (num << 3) | 7
            }
            0x300 => {
                let mapping = self.eval_int_list(args[0], env).map_err(|err| {
                    TableGenError::new(format!(
                        "{} while evaluating ResolveArgCode Mapping",
                        err.message
                    ))
                })?;
                let arg_codes = self.eval_int_list(args[1], env).map_err(|err| {
                    TableGenError::new(format!(
                        "{} while evaluating ResolveArgCode ArgCodes",
                        err.message
                    ))
                })?;
                let num = list_int_at(&mapping, al, "ResolveArgCode Mapping")?;
                (num << 3) | list_int_at(&arg_codes, num, "ResolveArgCode ArgCodes")?
            }
            0x400 => self.eval_int(args[2], env).map_err(|err| {
                TableGenError::new(format!(
                    "{} while resolving EncNextArgA ACIdx for ax {ax:#x}",
                    err.message
                ))
            })?,
            0x500 => {
                let mapping = self.eval_int_list(args[0], env)?;
                list_int_at(&mapping, al, "ResolveArgCode Mapping")?
            }
            _ => al,
        };
        Ok(Some(Value::Int(ret)))
    }

    pub(crate) fn eval_int_list(&mut self, expr: &Expr, env: &mut EnvStack) -> GenResult<Vec<i64>> {
        let mut out = Vec::new();
        for value in self.eval_to_list(expr, env)? {
            out.push(self.value_to_int(value)?);
        }
        Ok(out)
    }

    pub(crate) fn eval(&mut self, expr: &Expr, env: &mut EnvStack) -> GenResult<Value> {
        match expr {
            Expr::Ident(name) => {
                if let Some(value) = env.get(name) {
                    Ok(value)
                } else if let Some(record) = self.records.get(name) {
                    Ok(Value::Record(record.clone()))
                } else {
                    Ok(Value::String(name.clone()))
                }
            }
            Expr::String(s) => Ok(Value::String(s.clone())),
            Expr::Int(i) => Ok(Value::Int(*i)),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Unset => Ok(Value::Unset),
            Expr::List(items) => items
                .iter()
                .map(|item| self.eval(item, env))
                .collect::<GenResult<Vec<_>>>()
                .map(Value::List),
            Expr::Apply(app) => self.instantiate_class(app, None, env).map(Value::Record),
            Expr::Field(base, field) => {
                if let Some(value) = self.eval_synthetic_field(base, field, env)? {
                    return Ok(value);
                }
                let value = self.eval(base, env)?;
                self.field_value(value, field)
            }
            Expr::Index(base, idx) => {
                let value = self.eval(base, env)?;
                let idx = self.eval_int(idx, env)?;
                self.index_value(value, idx)
            }
            Expr::Concat(lhs, rhs) => {
                let lhs = self.eval(lhs, env)?;
                let rhs = self.eval(rhs, env)?;
                match (lhs, rhs) {
                    (Value::List(mut lhs), Value::List(rhs)) => {
                        lhs.extend(rhs);
                        Ok(Value::List(lhs))
                    }
                    (lhs, rhs) => Ok(Value::String(format!(
                        "{}{}",
                        self.value_to_string(lhs)?,
                        self.value_to_string(rhs)?
                    ))),
                }
            }
            Expr::RangeInclusive(start, end) => {
                let start = self.eval_int(start, env)?;
                let end = self.eval_int(end, env)?;
                Ok(Value::List((start..=end).map(Value::Int).collect()))
            }
            Expr::Bang(bang) => self.eval_bang(bang, env),
        }
    }

    pub(crate) fn eval_bang(&mut self, bang: &BangExpr, env: &mut EnvStack) -> GenResult<Value> {
        match bang {
            BangExpr::Call { op, type_arg, args } => {
                self.eval_bang_call(op, type_arg.as_deref(), args, env)
            }
            BangExpr::Cond(pairs) => {
                for (cond, value) in pairs {
                    if self.eval_bool(cond, env)? {
                        return self.eval(value, env);
                    }
                }
                Ok(Value::Unset)
            }
            BangExpr::Foreach(var, list, body) => {
                let values = self.eval_to_list(list, env)?;
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    env.push();
                    env.set(var.clone(), value);
                    out.push(self.eval(body, env)?);
                    env.pop();
                }
                Ok(Value::List(out))
            }
            BangExpr::Filter(var, list, pred) => {
                let values = self.eval_to_list(list, env)?;
                let mut out = Vec::new();
                for value in values {
                    env.push();
                    env.set(var.clone(), value.clone());
                    if self.eval_bool(pred, env)? {
                        out.push(value);
                    }
                    env.pop();
                }
                Ok(Value::List(out))
            }
            BangExpr::Foldl(init, list, acc, var, body) => {
                let mut accumulator = self.eval(init, env)?;
                let values = self.eval_to_list(list, env)?;
                for value in values {
                    env.push();
                    env.set(acc.clone(), accumulator);
                    env.set(var.clone(), value);
                    accumulator = self.eval(body, env)?;
                    env.pop();
                }
                Ok(accumulator)
            }
        }
    }

    pub(crate) fn eval_variadic_and(
        &mut self,
        args: &[Expr],
        env: &mut EnvStack,
    ) -> GenResult<Value> {
        let values = args
            .iter()
            .map(|arg| self.eval(arg, env))
            .collect::<GenResult<Vec<_>>>()?;
        if values.iter().all(|value| matches!(value, Value::Bool(_))) {
            return Ok(Value::Bool(
                values
                    .iter()
                    .all(|value| matches!(value, Value::Bool(true))),
            ));
        }
        let mut acc = !0_i64;
        for value in values {
            acc &= self.value_to_int(value)?;
        }
        Ok(Value::Int(acc))
    }

    pub(crate) fn eval_variadic_or(
        &mut self,
        args: &[Expr],
        env: &mut EnvStack,
    ) -> GenResult<Value> {
        let values = args
            .iter()
            .map(|arg| self.eval(arg, env))
            .collect::<GenResult<Vec<_>>>()?;
        if values.iter().all(|value| matches!(value, Value::Bool(_))) {
            return Ok(Value::Bool(
                values
                    .iter()
                    .any(|value| matches!(value, Value::Bool(true))),
            ));
        }
        let mut acc = 0_i64;
        for value in values {
            acc |= self.value_to_int(value)?;
        }
        Ok(Value::Int(acc))
    }

    pub(crate) fn value_to_int(&self, value: Value) -> GenResult<i64> {
        match value {
            Value::Int(i) => Ok(i),
            Value::Bool(b) => Ok(if b { 1 } else { 0 }),
            other => Err(TableGenError::new(format!(
                "expected integer, got {other:?}"
            ))),
        }
    }

    pub(crate) fn eval_bang_call(
        &mut self,
        op: &str,
        type_arg: Option<&str>,
        args: &[Expr],
        env: &mut EnvStack,
    ) -> GenResult<Value> {
        match op {
            "add" => Ok(Value::Int(
                self.eval_int(&args[0], env)? + self.eval_int(&args[1], env)?,
            )),
            "sub" => Ok(Value::Int(
                self.eval_int(&args[0], env)? - self.eval_int(&args[1], env)?,
            )),
            "mul" => Ok(Value::Int(
                self.eval_int(&args[0], env)? * self.eval_int(&args[1], env)?,
            )),
            "div" => Ok(Value::Int(
                self.eval_int(&args[0], env)? / self.eval_int(&args[1], env)?,
            )),
            "and" => self.eval_variadic_and(args, env),
            "or" => self.eval_variadic_or(args, env),
            "xor" => Ok(Value::Int(
                self.eval_int(&args[0], env)? ^ self.eval_int(&args[1], env)?,
            )),
            "shl" => Ok(Value::Int(
                self.eval_int(&args[0], env)? << self.eval_int(&args[1], env)?,
            )),
            "not" => Ok(Value::Bool(!self.eval_bool(&args[0], env)?)),
            "eq" => {
                let lhs = self.eval(&args[0], env)?;
                let rhs = self.eval(&args[1], env)?;
                Ok(Value::Bool(self.value_eq(lhs, rhs)))
            }
            "ne" => {
                let lhs = self.eval(&args[0], env)?;
                let rhs = self.eval(&args[1], env)?;
                Ok(Value::Bool(!self.value_eq(lhs, rhs)))
            }
            "le" => Ok(Value::Bool(
                self.eval_int(&args[0], env)? <= self.eval_int(&args[1], env)?,
            )),
            "lt" => Ok(Value::Bool(
                self.eval_int(&args[0], env)? < self.eval_int(&args[1], env)?,
            )),
            "ge" => Ok(Value::Bool(
                self.eval_int(&args[0], env)? >= self.eval_int(&args[1], env)?,
            )),
            "gt" => Ok(Value::Bool(
                self.eval_int(&args[0], env)? > self.eval_int(&args[1], env)?,
            )),
            "if" => {
                if self.eval_bool(&args[0], env)? {
                    self.eval(&args[1], env)
                } else {
                    self.eval(&args[2], env)
                }
            }
            "listconcat" => {
                let mut out = Vec::new();
                for arg in args {
                    out.extend(self.eval_to_list(arg, env)?);
                }
                Ok(Value::List(out))
            }
            "listsplat" => {
                let value = self.eval(&args[0], env)?;
                let count = self.eval_int(&args[1], env)?;
                Ok(Value::List((0..count).map(|_| value.clone()).collect()))
            }
            "listflatten" => {
                let mut out = Vec::new();
                for item in self.eval_to_list(&args[0], env)? {
                    match item {
                        Value::List(items) => out.extend(items),
                        other => out.push(other),
                    }
                }
                Ok(Value::List(out))
            }
            "substr" => {
                let source = self.eval_to_string(&args[0], env)?;
                let start = self.eval_int(&args[1], env)? as usize;
                let len = if args.len() > 2 {
                    Some(self.eval_int(&args[2], env)? as usize)
                } else {
                    None
                };
                let chars = source.chars().skip(start);
                let result: String = if let Some(len) = len {
                    chars.take(len).collect()
                } else {
                    chars.collect()
                };
                Ok(Value::String(result))
            }
            "size" => match self.eval(&args[0], env)? {
                Value::List(items) => Ok(Value::Int(items.len() as i64)),
                Value::String(s) => Ok(Value::Int(s.len() as i64)),
                other => Err(TableGenError::new(format!(
                    "!size unsupported for {other:?}"
                ))),
            },
            "empty" => match self.eval(&args[0], env)? {
                Value::List(items) => Ok(Value::Bool(items.is_empty())),
                Value::String(s) => Ok(Value::Bool(s.is_empty())),
                Value::Unset => Ok(Value::Bool(true)),
                other => Err(TableGenError::new(format!(
                    "!empty unsupported for {other:?}"
                ))),
            },
            "range" => {
                if args.len() == 1 {
                    match self.eval(&args[0], env)? {
                        Value::List(items) => Ok(Value::List(
                            (0..items.len() as i64).map(Value::Int).collect(),
                        )),
                        Value::Int(end) => Ok(Value::List((0..end).map(Value::Int).collect())),
                        other => Err(TableGenError::new(format!(
                            "!range unsupported for {other:?}"
                        ))),
                    }
                } else {
                    let start = self.eval_int(&args[0], env)?;
                    let end = self.eval_int(&args[1], env)?;
                    Ok(Value::List((start..end).map(Value::Int).collect()))
                }
            }
            "tail" => {
                let mut values = self.eval_to_list(&args[0], env)?;
                if !values.is_empty() {
                    values.remove(0);
                }
                Ok(Value::List(values))
            }
            "head" => {
                let values = self.eval_to_list(&args[0], env)?;
                Ok(values.into_iter().next().unwrap_or(Value::Unset))
            }
            "strconcat" => {
                let mut out = String::new();
                for arg in args {
                    out.push_str(&self.eval_to_string(arg, env)?);
                }
                Ok(Value::String(out))
            }
            "subst" => {
                let needle = self.eval_to_string(&args[0], env)?;
                let replacement = self.eval_to_string(&args[1], env)?;
                let haystack = self.eval_to_string(&args[2], env)?;
                Ok(Value::String(haystack.replace(&needle, &replacement)))
            }
            "find" => {
                let haystack = self.eval_to_string(&args[0], env)?;
                let needle = self.eval_to_string(&args[1], env)?;
                Ok(Value::Int(
                    haystack.find(&needle).map_or(-1, |idx| idx as i64),
                ))
            }
            "isa" => {
                let ty =
                    type_arg.ok_or_else(|| TableGenError::new("!isa requires a type argument"))?;
                let value = self.eval(&args[0], env)?;
                Ok(Value::Bool(self.value_is_a(&value, ty)))
            }
            "cast" => {
                let ty =
                    type_arg.ok_or_else(|| TableGenError::new("!cast requires a type argument"))?;
                let value = self.eval(&args[0], env)?;
                self.cast_value(value, ty)
            }
            other => Err(TableGenError::new(format!(
                "unsupported bang operator `!{other}`"
            ))),
        }
    }

    pub(crate) fn field_value(&self, value: Value, field: &str) -> GenResult<Value> {
        match value {
            Value::Record(record) => record.fields.get(field).cloned().ok_or_else(|| {
                TableGenError::new(format!("record {:?} has no field `{field}`", record.name))
            }),
            Value::SelfRef(name, _) if field == "NAME" => Ok(Value::String(name)),
            Value::String(s) if field == "NAME" => Ok(Value::String(s)),
            other => Err(TableGenError::new(format!(
                "cannot read field `{field}` from {other:?}"
            ))),
        }
    }

    pub(crate) fn index_value(&self, value: Value, index: i64) -> GenResult<Value> {
        let index =
            usize::try_from(index).map_err(|_| TableGenError::new("negative list index"))?;
        match value {
            Value::List(items) => items
                .get(index)
                .cloned()
                .ok_or_else(|| TableGenError::new(format!("list index {index} out of bounds"))),
            Value::String(s) => s
                .chars()
                .nth(index)
                .map(|ch| Value::String(ch.to_string()))
                .ok_or_else(|| TableGenError::new(format!("string index {index} out of bounds"))),
            other => Err(TableGenError::new(format!("cannot index {other:?}"))),
        }
    }

    pub(crate) fn cast_value(&self, value: Value, ty: &str) -> GenResult<Value> {
        match value {
            Value::Record(record) => {
                if record.classes.contains(ty) || record.name.as_deref() == Some(ty) {
                    Ok(Value::Record(record))
                } else {
                    Err(TableGenError::new(format!(
                        "record {:?} is not a `{ty}`",
                        record.name
                    )))
                }
            }
            Value::String(name) => {
                if let Some(record) = self.records.get(&name) {
                    if record.classes.contains(ty) || record.name.as_deref() == Some(ty) {
                        Ok(Value::Record(record.clone()))
                    } else {
                        Err(TableGenError::new(format!(
                            "record `{name}` is not a `{ty}`"
                        )))
                    }
                } else {
                    Ok(Value::SelfRef(name, ty.to_owned()))
                }
            }
            other => Err(TableGenError::new(format!(
                "cannot !cast {other:?} to `{ty}`"
            ))),
        }
    }

    pub(crate) fn value_is_a(&self, value: &Value, ty: &str) -> bool {
        match value {
            Value::Record(record) => {
                record.classes.contains(ty) || record.name.as_deref() == Some(ty)
            }
            Value::SelfRef(_, class) => class == ty,
            _ => false,
        }
    }

    pub(crate) fn eval_bool(&mut self, expr: &Expr, env: &mut EnvStack) -> GenResult<bool> {
        match self.eval(expr, env)? {
            Value::Bool(b) => Ok(b),
            Value::Int(i) => Ok(i != 0),
            Value::String(s) => Ok(!s.is_empty()),
            Value::List(items) => Ok(!items.is_empty()),
            Value::Unset => Ok(false),
            Value::Record(_) | Value::SelfRef(_, _) => Ok(true),
        }
    }

    pub(crate) fn eval_int(&mut self, expr: &Expr, env: &mut EnvStack) -> GenResult<i64> {
        match self.eval(expr, env)? {
            Value::Int(i) => Ok(i),
            Value::Bool(b) => Ok(if b { 1 } else { 0 }),
            Value::String(s) => s.parse::<i64>().map_err(|source| {
                TableGenError::new(format!("expected integer, got `{s}`: {source}"))
            }),
            other => Err(TableGenError::new(format!(
                "expected integer, got {other:?}"
            ))),
        }
    }

    pub(crate) fn eval_to_string(&mut self, expr: &Expr, env: &mut EnvStack) -> GenResult<String> {
        let value = match expr {
            Expr::Ident(name) if env.get(name).is_none() && !self.records.contains_key(name) => {
                Value::String(name.clone())
            }
            _ => self.eval(expr, env)?,
        };
        self.value_to_string(value)
    }

    pub(crate) fn eval_name(&mut self, expr: &Expr, env: &mut EnvStack) -> GenResult<String> {
        self.eval_to_string(expr, env)
    }

    pub(crate) fn value_to_string(&self, value: Value) -> GenResult<String> {
        match value {
            Value::String(s) => Ok(s),
            Value::Int(i) => Ok(i.to_string()),
            Value::Bool(b) => Ok(if b { "1" } else { "0" }.to_owned()),
            Value::Record(record) => record.name.clone().ok_or_else(|| {
                TableGenError::new("anonymous record cannot be converted to string")
            }),
            Value::SelfRef(name, _) => Ok(name),
            Value::Unset => Ok(String::new()),
            Value::List(_) => Err(TableGenError::new("list cannot be converted to string")),
        }
    }

    pub(crate) fn eval_to_list(
        &mut self,
        expr: &Expr,
        env: &mut EnvStack,
    ) -> GenResult<Vec<Value>> {
        match self.eval(expr, env)? {
            Value::List(values) => Ok(values),
            other => Err(TableGenError::new(format!("expected list, got {other:?}"))),
        }
    }

    pub(crate) fn value_eq(&self, lhs: Value, rhs: Value) -> bool {
        match (lhs, rhs) {
            (Value::Unset, Value::Unset) => true,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Int(a), Value::Bool(b)) | (Value::Bool(b), Value::Int(a)) => (a != 0) == b,
            (Value::Record(a), Value::Record(b)) => {
                if a.name.is_some() || b.name.is_some() {
                    a.name == b.name
                } else {
                    a.classes == b.classes && a.fields.len() == b.fields.len()
                }
            }
            (Value::Record(a), Value::String(b)) | (Value::String(b), Value::Record(a)) => {
                a.name.as_deref() == Some(b.as_str())
            }
            (Value::List(a), Value::List(b)) => {
                a.len() == b.len() && a.into_iter().zip(b).all(|(a, b)| self.value_eq(a, b))
            }
            _ => false,
        }
    }

    pub(crate) fn alloc_record_id(&self) -> usize {
        let id = self.next_record_id.get();
        self.next_record_id.set(id + 1);
        id
    }

    pub(crate) fn finish(&mut self) -> GenResult<String> {
        let mut intrinsics = self.collect_intrinsics()?;
        intrinsics.sort_by(|lhs, rhs| {
            (lhs.target_prefix.is_empty(), &lhs.name, lhs.record_id).cmp(&(
                rhs.target_prefix.is_empty(),
                &rhs.name,
                rhs.record_id,
            ))
        });
        // The tuple above sorted target intrinsics first because false < true; flip by stable key.
        intrinsics.sort_by(|lhs, rhs| {
            let lhs_has_target = !lhs.target_prefix.is_empty();
            let rhs_has_target = !rhs.target_prefix.is_empty();
            (lhs_has_target, &lhs.name, lhs.record_id).cmp(&(
                rhs_has_target,
                &rhs.name,
                rhs.record_id,
            ))
        });
        self.check_duplicates(&intrinsics)?;
        let target_sets = build_target_sets(&intrinsics);
        let required_targets = [
            "",
            "aarch64",
            "amdgcn",
            "arm",
            "bpf",
            "dx",
            "hexagon",
            "loongarch",
            "mips",
            "nvvm",
            "ppc",
            "r600",
            "riscv",
            "s390",
            "spv",
            "ve",
            "wasm",
            "x86",
            "xcore",
        ];
        let actual_targets = target_sets
            .iter()
            .map(|set| set.prefix.as_str())
            .collect::<Vec<_>>();
        if actual_targets != required_targets {
            return Err(TableGenError::new(format!(
                "target partition mismatch: expected {:?}, got {:?}",
                required_targets, actual_targets
            )));
        }
        render_generated(&intrinsics, &target_sets)
    }

    pub(crate) fn collect_intrinsics(&mut self) -> GenResult<Vec<IntrinsicOut>> {
        let default_properties = self
            .record_order
            .iter()
            .filter(|record| record.classes.contains("IntrinsicProperty"))
            .filter(|record| {
                field_bool(record, "IsDefault")
                    .unwrap_or(None)
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();

        let mut out = Vec::new();
        for record in &self.record_order {
            if !record.classes.contains("Intrinsic") {
                continue;
            }
            let record_name = record
                .name
                .as_deref()
                .ok_or_else(|| TableGenError::new("anonymous intrinsic record"))?;
            if !record_name.starts_with("int_") {
                return Err(TableGenError::new(format!(
                    "intrinsic record `{record_name}` does not start with int_"
                )));
            }
            let enum_name = record_name.trim_start_matches("int_").to_owned();
            let target_prefix = field_string(record, "TargetPrefix")?.unwrap_or_default();
            let explicit_name = field_string(record, "LLVMName")?.unwrap_or_default();
            let default_name = format!("llvm.{}", enum_name.replace('_', "."));
            let name = if explicit_name.is_empty() {
                default_name
            } else {
                if !explicit_name.starts_with("llvm.") {
                    return Err(TableGenError::new(format!(
                        "intrinsic `{record_name}` explicit name `{explicit_name}` does not start with llvm."
                    )));
                }
                explicit_name
            };
            if !target_prefix.is_empty() {
                let expected = format!("llvm.{target_prefix}.");
                if !name.starts_with(&expected) {
                    return Err(TableGenError::new(format!(
                        "target intrinsic `{record_name}` name `{name}` does not start with `{expected}`"
                    )));
                }
            }
            let ret_types = field_list(record, "RetTypes")?;
            let param_types = field_list(record, "ParamTypes")?;
            let mut properties = field_list(record, "IntrProperties")?;
            if !field_bool(record, "DisableDefaultAttributes")?.unwrap_or(true) {
                properties.extend(default_properties.iter().cloned().map(Value::Record));
            }
            let attr_info = compute_attrs(&properties)?;
            let type_sig = compute_type_signature(&ret_types, &param_types)?;
            let samples = compute_sample_overloads(&ret_types, &param_types)?;
            let clang_builtin = field_string(record, "ClangBuiltinName")?.filter(|s| !s.is_empty());
            let ms_builtin = field_string(record, "MSBuiltinName")?.filter(|s| !s.is_empty());
            out.push(IntrinsicOut {
                enum_name,
                name,
                target_prefix,
                overloaded: samples.is_some(),
                type_sig,
                fn_attrs: attr_info.fn_attrs,
                arg_attrs: attr_info.arg_attrs,
                memory_effects: attr_info.memory_effects,
                clang_builtin,
                ms_builtin,
                pretty_print: attr_info.pretty_print,
                sample_overloads: samples.unwrap_or_default(),
                record_id: record.id,
            });
        }
        Ok(out)
    }

    pub(crate) fn check_duplicates(&self, intrinsics: &[IntrinsicOut]) -> GenResult<()> {
        for pair in intrinsics.windows(2) {
            if pair[0].name == pair[1].name {
                return Err(TableGenError::new(format!(
                    "duplicate intrinsic name `{}` from `{}` and `{}`",
                    pair[0].name, pair[0].enum_name, pair[1].enum_name
                )));
            }
        }
        Ok(())
    }
}

pub(crate) fn as_record(value: &Value) -> GenResult<Rc<RecordValue>> {
    match value {
        Value::Record(record) => Ok(record.clone()),
        other => Err(TableGenError::new(format!(
            "expected record, got {other:?}"
        ))),
    }
}

pub(crate) fn field_list(record: &RecordValue, field: &str) -> GenResult<Vec<Value>> {
    list_field(record, field)
}

pub(crate) fn list_field(record: &RecordValue, field: &str) -> GenResult<Vec<Value>> {
    match record.fields.get(field) {
        Some(Value::List(items)) => Ok(items.clone()),
        Some(other) => Err(TableGenError::new(format!(
            "field `{field}` of {:?} is not a list: {other:?}",
            record.name
        ))),
        None => Ok(Vec::new()),
    }
}

pub(crate) fn field_bool(record: &RecordValue, field: &str) -> GenResult<Option<bool>> {
    match record.fields.get(field) {
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(Value::Int(i)) => Ok(Some(*i != 0)),
        Some(Value::Unset) | None => Ok(None),
        Some(other) => Err(TableGenError::new(format!(
            "field `{field}` of {:?} is not bool: {other:?}",
            record.name
        ))),
    }
}

pub(crate) fn field_string(record: &RecordValue, field: &str) -> GenResult<Option<String>> {
    string_field(record, field)
}

pub(crate) fn string_field(record: &RecordValue, field: &str) -> GenResult<Option<String>> {
    match record.fields.get(field) {
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(Value::Unset) | None => Ok(None),
        Some(other) => Err(TableGenError::new(format!(
            "field `{field}` of {:?} is not string: {other:?}",
            record.name
        ))),
    }
}

pub(crate) fn int_field(record: &RecordValue, field: &str) -> GenResult<i64> {
    match record.fields.get(field) {
        Some(Value::Int(i)) => Ok(*i),
        Some(Value::Bool(b)) => Ok(if *b { 1 } else { 0 }),
        Some(other) => Err(TableGenError::new(format!(
            "field `{field}` of {:?} is not int: {other:?}",
            record.name
        ))),
        None => Err(TableGenError::new(format!(
            "field `{field}` of {:?} is missing",
            record.name
        ))),
    }
}

pub(crate) fn record_field_record(record: &RecordValue, field: &str) -> GenResult<Rc<RecordValue>> {
    match record.fields.get(field) {
        Some(Value::Record(value)) => Ok(value.clone()),
        Some(other) => Err(TableGenError::new(format!(
            "field `{field}` of {:?} is not record: {other:?}",
            record.name
        ))),
        None => Err(TableGenError::new(format!(
            "field `{field}` of {:?} is missing",
            record.name
        ))),
    }
}

/// Builds a `RecordValue` for the generator tests in this crate.
///
/// Shared because the property tests in `basic::code_gen_intrinsics` and
/// the sample-overload tests in `basic::intrinsic_emitter` both need one.
#[cfg(test)]
pub(crate) fn test_record(name: &str, classes: &[&str], fields: &[(&str, Value)]) -> Value {
    let mut builder = RecordBuilder {
        name: Some(name.to_owned()),
        ..RecordBuilder::default()
    };
    for class in classes {
        builder.classes.insert((*class).to_owned());
    }
    for (name, value) in fields {
        builder.fields.insert((*name).to_owned(), value.clone());
    }
    Value::Record(builder.finish(0))
}
