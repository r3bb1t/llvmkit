//! The TableGen parser and its syntax tree.
//!
//! Ports `llvm/lib/TableGen/TGParser.cpp`.

use crate::*;

#[derive(Debug, Clone)]
pub(crate) struct Located<T> {
    pub(crate) node: T,
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) col: usize,
}

impl<T> Located<T> {
    pub(crate) fn new(node: T, token: &Token) -> Self {
        Self {
            node,
            file: token.file.clone(),
            line: token.line,
            col: token.col,
        }
    }

    pub(crate) fn error<M>(&self, message: M) -> TableGenError
    where
        M: Into<String>,
    {
        TableGenError::new(format!(
            "{}:{}:{}: {}",
            self.file,
            self.line,
            self.col,
            message.into()
        ))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Stmt {
    Include(String),
    Class(TemplateDef),
    MultiClass(TemplateDef),
    Def(DefStmt),
    Defm(DefmStmt),
    Defvar(String, Expr),
    Defset(String, String, Vec<Located<Stmt>>),
    LetBlock(Vec<(String, Expr)>, Vec<Located<Stmt>>),
    Foreach(String, Expr, Vec<Located<Stmt>>),
    If(Expr, Vec<Located<Stmt>>, Vec<Located<Stmt>>),
    Field(String, Expr),
    Assert(Expr, Expr),
}

#[derive(Debug, Clone)]
pub(crate) struct TemplateDef {
    pub(crate) name: String,
    pub(crate) params: Vec<TemplateParam>,
    pub(crate) parents: Vec<Apply>,
    pub(crate) body: Vec<Located<Stmt>>,
}

#[derive(Debug, Clone)]
pub(crate) struct TemplateParam {
    pub(crate) name: String,
    pub(crate) default: Option<Expr>,
}

#[derive(Debug, Clone)]
pub(crate) struct DefStmt {
    pub(crate) name: Expr,
    pub(crate) parents: Vec<Apply>,
    pub(crate) body: Vec<Located<Stmt>>,
}

#[derive(Debug, Clone)]
pub(crate) struct DefmStmt {
    pub(crate) name: Expr,
    pub(crate) parents: Vec<Apply>,
}

#[derive(Debug, Clone)]
pub(crate) struct Apply {
    pub(crate) name: String,
    pub(crate) args: Vec<TemplateArg>,
}

#[derive(Debug, Clone)]
pub(crate) enum TemplateArg {
    Pos(Expr),
    Named(String, Expr),
}

#[derive(Debug, Clone)]
pub(crate) enum Expr {
    Ident(String),
    String(String),
    Int(i64),
    Bool(bool),
    Unset,
    List(Vec<Expr>),
    Apply(Apply),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Concat(Box<Expr>, Box<Expr>),
    RangeInclusive(Box<Expr>, Box<Expr>),
    Bang(BangExpr),
}

impl Expr {
    pub(crate) fn contains_ident(&self, needle: &str) -> bool {
        match self {
            Expr::Ident(name) => name == needle,
            Expr::String(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Unset => false,
            Expr::List(items) => items.iter().any(|item| item.contains_ident(needle)),
            Expr::Apply(app) => app.args.iter().any(|arg| match arg {
                TemplateArg::Pos(expr) | TemplateArg::Named(_, expr) => expr.contains_ident(needle),
            }),
            Expr::Field(base, _) => base.contains_ident(needle),
            Expr::Index(base, idx) | Expr::Concat(base, idx) | Expr::RangeInclusive(base, idx) => {
                base.contains_ident(needle) || idx.contains_ident(needle)
            }
            Expr::Bang(bang) => bang.contains_ident(needle),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum BangExpr {
    Call {
        op: String,
        type_arg: Option<String>,
        args: Vec<Expr>,
    },
    Cond(Vec<(Expr, Expr)>),
    Foreach(String, Box<Expr>, Box<Expr>),
    Filter(String, Box<Expr>, Box<Expr>),
    Foldl(Box<Expr>, Box<Expr>, String, String, Box<Expr>),
}

impl BangExpr {
    pub(crate) fn contains_ident(&self, needle: &str) -> bool {
        match self {
            BangExpr::Call { args, .. } => args.iter().any(|arg| arg.contains_ident(needle)),
            BangExpr::Cond(pairs) => pairs
                .iter()
                .any(|(cond, value)| cond.contains_ident(needle) || value.contains_ident(needle)),
            BangExpr::Foreach(var, list, body) | BangExpr::Filter(var, list, body) => {
                var == needle || list.contains_ident(needle) || body.contains_ident(needle)
            }
            BangExpr::Foldl(init, list, acc, var, body) => {
                acc == needle
                    || var == needle
                    || init.contains_ident(needle)
                    || list.contains_ident(needle)
                    || body.contains_ident(needle)
            }
        }
    }
}

pub(crate) struct Parser {
    pub(crate) tokens: Vec<Token>,
    pub(crate) pos: usize,
}

impl Parser {
    pub(crate) fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub(crate) fn parse(mut self) -> GenResult<Vec<Located<Stmt>>> {
        let mut stmts = Vec::new();
        while !self.is_eof() {
            if self.eat(TokenKind::RBrace) {
                continue;
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    pub(crate) fn parse_stmt(&mut self) -> GenResult<Located<Stmt>> {
        let token = self
            .peek()
            .ok_or_else(|| TableGenError::new("unexpected EOF"))?
            .clone();
        let stmt = if self.eat_ident("include") {
            let path = self.expect_string()?;
            Stmt::Include(path)
        } else if self.eat_ident("class") {
            Stmt::Class(self.parse_template_def()?)
        } else if self.eat_ident("multiclass") {
            Stmt::MultiClass(self.parse_template_def()?)
        } else if self.eat_ident("defm") {
            Stmt::Defm(self.parse_defm_after_keyword()?)
        } else if self.eat_ident("defvar") {
            let name = self.expect_ident()?;
            self.expect(TokenKind::Equal)?;
            let expr = self.parse_expr()?;
            self.expect(TokenKind::Semi)?;
            Stmt::Defvar(name, expr)
        } else if self.eat_ident("defset") {
            self.parse_defset_after_keyword()?
        } else if self.eat_ident("def") {
            Stmt::Def(self.parse_def_after_keyword()?)
        } else if self.eat_ident("foreach") {
            self.parse_foreach_after_keyword()?
        } else if self.eat_ident("let") {
            self.parse_let_after_keyword()?
        } else if self.eat_ident("if") {
            self.parse_if_after_keyword()?
        } else if self.eat_ident("assert") {
            let cond = self.parse_expr()?;
            self.expect(TokenKind::Comma)?;
            let msg = self.parse_expr()?;
            self.expect(TokenKind::Semi)?;
            Stmt::Assert(cond, msg)
        } else {
            self.parse_field_stmt()?
        };
        Ok(Located::new(stmt, &token))
    }

    pub(crate) fn parse_template_def(&mut self) -> GenResult<TemplateDef> {
        let name = self.expect_ident()?;
        let params = if self.eat(TokenKind::Less) {
            self.parse_template_params()?
        } else {
            Vec::new()
        };
        let parents = if self.eat(TokenKind::Colon) {
            self.parse_apply_list_until_body()?
        } else {
            Vec::new()
        };
        let body = if self.eat(TokenKind::LBrace) {
            self.parse_block_body()?
        } else {
            self.expect(TokenKind::Semi)?;
            Vec::new()
        };
        Ok(TemplateDef {
            name,
            params,
            parents,
            body,
        })
    }

    pub(crate) fn parse_template_params(&mut self) -> GenResult<Vec<TemplateParam>> {
        let mut params = Vec::new();
        if self.eat(TokenKind::Greater) {
            return Ok(params);
        }
        loop {
            let name = self.parse_typed_name_before(&[
                TokenKind::Equal,
                TokenKind::Comma,
                TokenKind::Greater,
            ])?;
            let default = if self.eat(TokenKind::Equal) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            params.push(TemplateParam { name, default });
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::Greater)?;
            break;
        }
        Ok(params)
    }

    pub(crate) fn parse_apply_list_until_body(&mut self) -> GenResult<Vec<Apply>> {
        let mut parents = Vec::new();
        loop {
            parents.push(self.parse_apply()?);
            if self.eat(TokenKind::Comma) {
                continue;
            }
            break;
        }
        Ok(parents)
    }

    pub(crate) fn parse_def_after_keyword(&mut self) -> GenResult<DefStmt> {
        let name = self.parse_expr()?;
        let parents = if self.eat(TokenKind::Colon) {
            self.parse_apply_list_until_body()?
        } else {
            Vec::new()
        };
        let body = if self.eat(TokenKind::LBrace) {
            self.parse_block_body()?
        } else {
            self.expect(TokenKind::Semi)?;
            Vec::new()
        };
        Ok(DefStmt {
            name,
            parents,
            body,
        })
    }

    pub(crate) fn parse_defm_after_keyword(&mut self) -> GenResult<DefmStmt> {
        let name = self.parse_expr()?;
        self.expect(TokenKind::Colon)?;
        let parents = self.parse_apply_list_until_body()?;
        self.expect(TokenKind::Semi)?;
        Ok(DefmStmt { name, parents })
    }

    pub(crate) fn parse_defset_after_keyword(&mut self) -> GenResult<Stmt> {
        let type_name = self.parse_defset_type_name()?;
        let name = self.expect_ident()?;
        self.expect(TokenKind::Equal)?;
        self.expect(TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::Defset(name, type_name, body))
    }

    pub(crate) fn parse_defset_type_name(&mut self) -> GenResult<String> {
        if self.eat_ident("list") {
            self.expect(TokenKind::Less)?;
            let name = self.expect_ident()?;
            self.skip_balanced_type_tail()?;
            Ok(name)
        } else {
            self.expect_ident()
        }
    }

    pub(crate) fn skip_balanced_type_tail(&mut self) -> GenResult<()> {
        let mut depth = 1usize;
        while depth > 0 {
            let token = self
                .next()
                .ok_or_else(|| self.error_here("unexpected EOF in type"))?;
            match token.kind.clone() {
                TokenKind::Less => depth += 1,
                TokenKind::Greater => depth -= 1,
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn parse_foreach_after_keyword(&mut self) -> GenResult<Stmt> {
        let var = self.expect_ident()?;
        self.expect(TokenKind::Equal)?;
        let mut expr = self.parse_expr()?;
        if self.eat(TokenKind::Ellipsis) {
            let end = self.parse_expr()?;
            expr = Expr::RangeInclusive(Box::new(expr), Box::new(end));
        }
        self.expect_ident_value("in")?;
        let body = self.parse_stmt_or_block()?;
        Ok(Stmt::Foreach(var, expr, body))
    }

    pub(crate) fn parse_let_after_keyword(&mut self) -> GenResult<Stmt> {
        let assigns = self.parse_let_assignments()?;
        if self.eat_ident("in") {
            let body = self.parse_stmt_or_block()?;
            Ok(Stmt::LetBlock(assigns, body))
        } else if self.eat(TokenKind::Semi) {
            if assigns.len() != 1 {
                return Err(self.error_here("field let must contain exactly one assignment"));
            }
            let (name, expr) = assigns.into_iter().next().unwrap();
            Ok(Stmt::Field(name, expr))
        } else {
            let body = self.parse_stmt_or_block()?;
            Ok(Stmt::LetBlock(assigns, body))
        }
    }

    pub(crate) fn parse_let_assignments(&mut self) -> GenResult<Vec<(String, Expr)>> {
        let mut assigns = Vec::new();
        loop {
            let name = self.expect_ident()?;
            self.expect(TokenKind::Equal)?;
            let expr = self.parse_expr()?;
            assigns.push((name, expr));
            if self.eat(TokenKind::Comma) {
                continue;
            }
            break;
        }
        Ok(assigns)
    }

    pub(crate) fn parse_if_after_keyword(&mut self) -> GenResult<Stmt> {
        let cond = self.parse_expr()?;
        self.expect_ident_value("then")?;
        let then_body = self.parse_stmt_or_block()?;
        let else_body = if self.eat_ident("else") {
            self.parse_stmt_or_block()?
        } else {
            Vec::new()
        };
        Ok(Stmt::If(cond, then_body, else_body))
    }

    pub(crate) fn parse_field_stmt(&mut self) -> GenResult<Stmt> {
        let name = self.parse_typed_name_before(&[TokenKind::Equal])?;
        self.expect(TokenKind::Equal)?;
        let expr = self.parse_expr()?;
        if !self.eat(TokenKind::Semi)
            && !matches!(
                self.peek().map(|tok| &tok.kind),
                Some(TokenKind::RBrace | TokenKind::Ident(_))
            )
        {
            return Err(self.error_here("expected `;`"));
        }
        Ok(Stmt::Field(name, expr))
    }

    pub(crate) fn parse_typed_name_before(&mut self, stops: &[TokenKind]) -> GenResult<String> {
        let start = self.pos;
        let mut depth_angle = 0usize;
        let mut candidate = None;
        while let Some(token) = self.peek() {
            let at_stop =
                depth_angle == 0 && stops.iter().any(|stop| token.kind.same_variant(stop));
            if at_stop {
                break;
            }
            let token = self.next().unwrap();
            match token.kind.clone() {
                TokenKind::Less => depth_angle += 1,
                TokenKind::Greater => depth_angle = depth_angle.saturating_sub(1),
                TokenKind::Ident(name) if depth_angle == 0 => candidate = Some(name),
                _ => {}
            }
        }
        candidate.ok_or_else(|| {
            self.pos = start;
            self.error_here("expected typed name")
        })
    }

    pub(crate) fn parse_stmt_or_block(&mut self) -> GenResult<Vec<Located<Stmt>>> {
        if self.eat(TokenKind::LBrace) {
            self.parse_block_body()
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    pub(crate) fn parse_block_body(&mut self) -> GenResult<Vec<Located<Stmt>>> {
        let mut body = Vec::new();
        while !self.eat(TokenKind::RBrace) {
            if self.is_eof() {
                return Err(self.error_here("unexpected EOF in block"));
            }
            body.push(self.parse_stmt()?);
        }
        Ok(body)
    }

    pub(crate) fn parse_apply(&mut self) -> GenResult<Apply> {
        let name = self.expect_ident()?;
        let args = if self.eat(TokenKind::Less) {
            self.parse_template_args()?
        } else {
            Vec::new()
        };
        Ok(Apply { name, args })
    }

    pub(crate) fn parse_template_args(&mut self) -> GenResult<Vec<TemplateArg>> {
        let mut args = Vec::new();
        if self.eat(TokenKind::Greater) {
            return Ok(args);
        }
        loop {
            if let Some((name, expr)) = self.try_parse_named_arg()? {
                args.push(TemplateArg::Named(name, expr));
            } else {
                args.push(TemplateArg::Pos(self.parse_expr()?));
            }
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::Greater)?;
            break;
        }
        Ok(args)
    }

    pub(crate) fn try_parse_named_arg(&mut self) -> GenResult<Option<(String, Expr)>> {
        let save = self.pos;
        if let Some(Token {
            kind: TokenKind::Ident(name),
            ..
        }) = self.peek().cloned()
        {
            self.next();
            if self.eat(TokenKind::Equal) {
                let expr = self.parse_expr()?;
                return Ok(Some((name, expr)));
            }
        }
        self.pos = save;
        Ok(None)
    }

    pub(crate) fn parse_expr(&mut self) -> GenResult<Expr> {
        self.parse_concat()
    }

    pub(crate) fn parse_concat(&mut self) -> GenResult<Expr> {
        let mut expr = self.parse_postfix()?;
        while self.eat(TokenKind::Hash) {
            let rhs = self.parse_postfix()?;
            expr = Expr::Concat(Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    pub(crate) fn parse_postfix(&mut self) -> GenResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.eat(TokenKind::Dot) {
                let field = self.expect_ident()?;
                expr = Expr::Field(Box::new(expr), field);
            } else if self.eat(TokenKind::LBracket) {
                let index = self.parse_expr()?;
                self.expect(TokenKind::RBracket)?;
                expr = Expr::Index(Box::new(expr), Box::new(index));
            } else {
                break;
            }
        }
        Ok(expr)
    }

    pub(crate) fn parse_primary(&mut self) -> GenResult<Expr> {
        let token = self
            .next()
            .ok_or_else(|| TableGenError::new("unexpected EOF in expression"))?;
        match token.kind.clone() {
            TokenKind::Ident(name) => {
                if name == "true" {
                    Ok(Expr::Bool(true))
                } else if name == "false" {
                    Ok(Expr::Bool(false))
                } else if self.eat(TokenKind::Less) {
                    let args = self.parse_template_args_after_less()?;
                    Ok(Expr::Apply(Apply { name, args }))
                } else {
                    Ok(Expr::Ident(name))
                }
            }
            TokenKind::String(s) => Ok(Expr::String(s)),
            TokenKind::Int(i) => Ok(Expr::Int(i)),
            TokenKind::Question => Ok(Expr::Unset),
            TokenKind::LBracket => self.parse_list_after_lbracket(),
            TokenKind::LParen => {
                let expr = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                Ok(expr)
            }
            TokenKind::BangIdent(op) => self.parse_bang(op),
            other => Err(Located::new(other, &token).error("expected expression")),
        }
    }

    pub(crate) fn parse_template_args_after_less(&mut self) -> GenResult<Vec<TemplateArg>> {
        self.parse_template_args()
    }

    pub(crate) fn parse_list_after_lbracket(&mut self) -> GenResult<Expr> {
        let mut items = Vec::new();
        if self.eat(TokenKind::RBracket) {
            self.eat_type_suffix()?;
            return Ok(Expr::List(items));
        }
        loop {
            items.push(self.parse_expr()?);
            if self.eat(TokenKind::Comma) {
                if self.eat(TokenKind::RBracket) {
                    self.eat_type_suffix()?;
                    break;
                }
                continue;
            }
            self.expect(TokenKind::RBracket)?;
            self.eat_type_suffix()?;
            break;
        }
        Ok(Expr::List(items))
    }

    pub(crate) fn eat_type_suffix(&mut self) -> GenResult<()> {
        if self.eat(TokenKind::Less) {
            let mut depth = 1usize;
            while depth > 0 {
                let token = self
                    .next()
                    .ok_or_else(|| self.error_here("unexpected EOF in list type suffix"))?;
                match token.kind.clone() {
                    TokenKind::Less => depth += 1,
                    TokenKind::Greater => depth -= 1,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub(crate) fn parse_bang(&mut self, op: String) -> GenResult<Expr> {
        let type_arg = if matches!(op.as_str(), "cast" | "isa") && self.eat(TokenKind::Less) {
            let ty = self.expect_ident()?;
            self.expect(TokenKind::Greater)?;
            Some(ty)
        } else {
            None
        };
        self.expect(TokenKind::LParen)?;
        let bang = match op.as_str() {
            "cond" => {
                let mut pairs = Vec::new();
                loop {
                    let cond = self.parse_expr()?;
                    self.expect(TokenKind::Colon)?;
                    let value = self.parse_expr()?;
                    pairs.push((cond, value));
                    if self.eat(TokenKind::Comma) {
                        if self.eat(TokenKind::RParen) {
                            break;
                        }
                        continue;
                    }
                    self.expect(TokenKind::RParen)?;
                    break;
                }
                BangExpr::Cond(pairs)
            }
            "foreach" | "filter" => {
                let var = self.expect_ident()?;
                self.expect(TokenKind::Comma)?;
                let list = self.parse_expr()?;
                self.expect(TokenKind::Comma)?;
                let body = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                if op == "foreach" {
                    BangExpr::Foreach(var, Box::new(list), Box::new(body))
                } else {
                    BangExpr::Filter(var, Box::new(list), Box::new(body))
                }
            }
            "foldl" => {
                let init = self.parse_expr()?;
                self.expect(TokenKind::Comma)?;
                let list = self.parse_expr()?;
                self.expect(TokenKind::Comma)?;
                let acc = self.expect_ident()?;
                self.expect(TokenKind::Comma)?;
                let var = self.expect_ident()?;
                self.expect(TokenKind::Comma)?;
                let body = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                BangExpr::Foldl(Box::new(init), Box::new(list), acc, var, Box::new(body))
            }
            _ => {
                let mut args = Vec::new();
                if self.eat(TokenKind::RParen) {
                    BangExpr::Call { op, type_arg, args }
                } else {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.eat(TokenKind::Comma) {
                            continue;
                        }
                        self.expect(TokenKind::RParen)?;
                        break;
                    }
                    BangExpr::Call { op, type_arg, args }
                }
            }
        };
        Ok(Expr::Bang(bang))
    }

    pub(crate) fn expect_string(&mut self) -> GenResult<String> {
        let token = self
            .next()
            .ok_or_else(|| self.error_here("expected string"))?;
        match token.kind.clone() {
            TokenKind::String(s) => Ok(s),
            other => Err(Located::new(other, &token).error("expected string")),
        }
    }

    pub(crate) fn expect_ident(&mut self) -> GenResult<String> {
        let token = self
            .next()
            .ok_or_else(|| self.error_here("expected identifier"))?;
        match token.kind.clone() {
            TokenKind::Ident(s) => Ok(s),
            other => Err(Located::new(other, &token).error("expected identifier")),
        }
    }

    pub(crate) fn expect_ident_value(&mut self, expected: &str) -> GenResult<()> {
        let got = self.expect_ident()?;
        if got == expected {
            Ok(())
        } else {
            Err(self.error_here(format!("expected `{expected}`, got `{got}`")))
        }
    }

    pub(crate) fn eat_ident(&mut self, expected: &str) -> bool {
        if matches!(self.peek().map(|t| &t.kind), Some(TokenKind::Ident(s)) if s == expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn eat(&mut self, kind: TokenKind) -> bool {
        if self.peek().is_some_and(|tok| tok.kind.same_variant(&kind)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, kind: TokenKind) -> GenResult<()> {
        let token = self
            .next()
            .ok_or_else(|| self.error_here("unexpected EOF"))?;
        if token.kind.same_variant(&kind) {
            Ok(())
        } else {
            Err(Located::new(token.kind.clone(), &token)
                .error(format!("expected `{}`", kind.display())))
        }
    }

    pub(crate) fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned()?;
        self.pos += 1;
        Some(token)
    }

    pub(crate) fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    pub(crate) fn is_eof(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    pub(crate) fn error_here<M>(&self, message: M) -> TableGenError
    where
        M: Into<String>,
    {
        if let Some(tok) = self.peek() {
            TableGenError::new(format!(
                "{}:{}:{}: {}",
                tok.file,
                tok.line,
                tok.col,
                message.into()
            ))
        } else {
            TableGenError::new(message.into())
        }
    }
}

impl TokenKind {
    pub(crate) fn same_variant(&self, other: &TokenKind) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    pub(crate) fn display(&self) -> &'static str {
        match self {
            TokenKind::Ident(_) => "identifier",
            TokenKind::BangIdent(_) => "bang identifier",
            TokenKind::String(_) => "string",
            TokenKind::Int(_) => "integer",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::Less => "<",
            TokenKind::Greater => ">",
            TokenKind::Colon => ":",
            TokenKind::Semi => ";",
            TokenKind::Comma => ",",
            TokenKind::Equal => "=",
            TokenKind::Dot => ".",
            TokenKind::Ellipsis => "...",
            TokenKind::Hash => "#",
            TokenKind::Question => "?",
        }
    }
}
