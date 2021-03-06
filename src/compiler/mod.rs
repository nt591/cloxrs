mod scanner;
mod token;

use plain_enum::TPlainEnum;

use super::chunk::{Chunk, OpCodeLine};
use super::opcode::OpCode;
use super::value::Value;
use scanner::Scanner;
use token::*;

pub struct Compiler<'a> {
    scanner: Scanner<'a>,
    compiling_chunk: &'a mut Chunk,

    previous: Token<'a>,
    current: Token<'a>,

    had_error: bool,
    panic_mode: bool,

    locals: Vec<Local<'a>>,
    scope_depth: usize,
}

impl<'a> Compiler<'a> {
    pub fn compile(&mut self, source: &'a String) -> bool {
        self.scanner = Scanner::new(source);
        self.advance();

        while !self.match_next(Type::EOF) {
            self.declaration();
        }

        self.end_compiliation();
        !self.had_error
    }

    fn expression(&mut self) {
        self.parse_precedence(Precedence::Assignment);
    }

    fn declaration(&mut self) {
        if self.match_next(Type::Var) {
            self.var_declaration();
        } else {
            self.statement();
        }

        if self.panic_mode {
            self.syncronize();
        }
    }

    fn var_declaration(&mut self) {
        let global = self.parse_variable("Expected variable name.");

        if self.match_next(Type::Equal) {
            self.expression();
        } else {
            self.emit_opcode(OpCode::Constant(Value::Nil));
        }

        self.consume(Type::Semicolon, "Expected ';' after variable declaration.");
        self.define_variable(global);
    }

    fn statement(&mut self) {
        match () {
            _ if self.match_next(Type::Print) => self.print_statement(),
            _ if self.match_next(Type::If) => self.if_statement(),
            _ if self.match_next(Type::While) => self.while_statement(),
            _ if self.match_next(Type::For) => self.for_statement(),
            _ if self.match_next(Type::LeftBrace) => {
                self.begin_scope();
                self.block();
                self.end_scope();
            }
            _ => self.expression_statement(),
        };
    }

    fn print_statement(&mut self) {
        self.expression();
        self.consume(Type::Semicolon, "Expected ';' after value.");
        self.emit_opcode(OpCode::Print);
    }

    fn if_statement(&mut self) {
        self.consume(Type::LeftParen, "Expected '(' after 'if'.");
        self.expression();
        self.consume(Type::RightParen, "Expected ')' after condition.");

        let then_jump = self.emit_jump(OpCode::JumpIfFalse(0));
        self.emit_opcode(OpCode::Pop);
        self.statement();
        let else_jump = self.emit_jump(OpCode::Jump(0));
        self.patch_jump(then_jump);
        self.emit_opcode(OpCode::Pop);

        if self.match_next(Type::Else) {
            self.statement();
        }
        self.patch_jump(else_jump);
    }

    fn while_statement(&mut self) {
        let loop_start = self.compiling_chunk.code.len();

        self.consume(Type::LeftParen, "Expected '(' after 'while'.");
        self.expression();
        self.consume(Type::RightParen, "Expected ')' after condition.");

        let exit_jump = self.emit_jump(OpCode::JumpIfFalse(0));

        self.emit_opcode(OpCode::Pop);
        self.statement();
        self.emit_loop(loop_start);

        self.patch_jump(exit_jump);
        self.emit_opcode(OpCode::Pop);
    }

    fn for_statement(&mut self) {
        self.begin_scope();

        self.consume(Type::LeftParen, "Expected '(' after 'for'.");

        if self.match_next(Type::Var) {
            self.var_declaration();
        } else if self.match_next(Type::Semicolon) {
        } else {
            self.expression_statement();
        }

        let mut loop_start = self.compiling_chunk.code.len();

        let mut exit_jump: usize = 0;
        if !self.match_next(Type::Semicolon) {
            self.expression();
            self.consume(Type::Semicolon, "Expected ';' after loop condition.");

            exit_jump = self.emit_jump(OpCode::JumpIfFalse(0));
            self.emit_opcode(OpCode::Pop);
        }

        if !self.match_next(Type::RightParen) {
            let body_jump = self.emit_jump(OpCode::Jump(0));

            let increment_start = self.compiling_chunk.code.len();
            self.expression();
            self.emit_opcode(OpCode::Pop);
            self.consume(Type::RightParen, "Expected ')' after for clauses.");

            self.emit_loop(loop_start);
            loop_start = increment_start;
            self.patch_jump(body_jump);
        }

        self.statement();

        self.emit_loop(loop_start);

        if exit_jump != 0 {
            self.patch_jump(exit_jump);
            self.emit_opcode(OpCode::Pop);
        }

        self.end_scope();
    }

    fn expression_statement(&mut self) {
        self.expression();
        self.consume(Type::Semicolon, "Expected ';' after expression.");
        self.emit_opcode(OpCode::Pop);
    }

    fn block(&mut self) {
        while !self.check(Type::RightBrace) && !self.check(Type::EOF) {
            self.declaration();
        }

        self.consume(Type::RightBrace, "Expected '}' after block.");
    }

    fn grouping(&mut self) {
        self.expression();
        self.consume(Type::RightParen, "Expected ')' after expression.");
    }

    fn unary(&mut self) {
        let op_type = self.previous.t_type;
        self.parse_precedence(Precedence::Unary);

        match op_type {
            Type::Minus => self.emit_opcode(OpCode::Negate),
            Type::Bang => self.emit_opcode(OpCode::Not),
            _ => (),
        }
    }

    fn binary(&mut self) {
        let op_type = self.previous.t_type;

        let rule = Compiler::get_rule(op_type);
        self.parse_precedence(Precedence::from_usize(rule.precedence.to_usize() + 1));

        match op_type {
            Type::Plus => self.emit_opcode(OpCode::Add),
            Type::Minus => self.emit_opcode(OpCode::Substract),
            Type::Star => self.emit_opcode(OpCode::Multiply),
            Type::Slash => self.emit_opcode(OpCode::Divide),

            Type::BangEqual => self.emit_opcodes(OpCode::Equal, OpCode::Not),
            Type::EqualEqual => self.emit_opcode(OpCode::Equal),
            Type::Greater => self.emit_opcode(OpCode::Greater),
            Type::GreaterEqual => self.emit_opcodes(OpCode::Less, OpCode::Not),
            Type::Less => self.emit_opcode(OpCode::Less),
            Type::LessEqual => self.emit_opcodes(OpCode::Greater, OpCode::Not),

            _ => (),
        }
    }

    fn and(&mut self) {
        let end_jump = self.emit_jump(OpCode::JumpIfFalse(0));
        self.emit_opcode(OpCode::Pop);
        self.parse_precedence(Precedence::And);
        self.patch_jump(end_jump);
    }

    fn or(&mut self) {
        let else_jump = self.emit_jump(OpCode::JumpIfFalse(0));
        let end_jump = self.emit_jump(OpCode::Jump(0));

        self.patch_jump(else_jump);
        self.emit_opcode(OpCode::Pop);

        self.parse_precedence(Precedence::Or);
        self.patch_jump(end_jump);
    }

    fn literal(&mut self) {
        match self.previous.t_type {
            Type::False => self.emit_opcode(OpCode::Constant(Value::Bool(false))),
            Type::Nil => self.emit_opcode(OpCode::Constant(Value::Nil)),
            Type::True => self.emit_opcode(OpCode::Constant(Value::Bool(true))),
            Type::Number => {
                let value: f64 = self.previous.lexeme.parse().expect("Invalid number?");
                self.emit_opcode(OpCode::Constant(Value::Number(value)));
            }
            _ => (),
        }
    }

    fn string(&mut self) {
        // Trim leading and trailing "
        let mut string = String::from(&self.previous.lexeme[1..]);
        string.pop();
        self.emit_opcode(OpCode::Constant(Value::String(string)))
    }

    fn variable(&mut self, can_assign: bool) {
        self.named_variable(self.previous, can_assign);
    }

    fn named_variable(&mut self, name: Token, can_assign: bool) {
        let get_op;
        let set_op;
        let arg = self.resolve_local(name);
        if let Some(arg) = arg {
            get_op = OpCode::GetLocal(arg);
            set_op = OpCode::SetLocal(arg);
        } else {
            let arg = self.identifier_constant(name);
            get_op = OpCode::GetGlobal(arg.clone());
            set_op = OpCode::SetGlobal(arg);
        }

        if can_assign && self.match_next(Type::Equal) {
            self.expression();
            self.emit_opcode(set_op);
        } else {
            self.emit_opcode(get_op);
        }
    }

    fn parse_precedence(&mut self, precedence: Precedence) {
        self.advance();
        let prefix_rule = Compiler::get_rule(self.previous.t_type).prefix;
        let can_assign = precedence <= Precedence::Assignment;

        if let Some(rule) = prefix_rule {
            rule(self, can_assign);
        } else {
            self.error_at(self.current, "Expected expression.");
            return;
        }

        while precedence.to_usize()
            <= Compiler::get_rule(self.current.t_type)
                .precedence
                .to_usize()
        {
            self.advance();
            let infix_rule = Compiler::get_rule(self.previous.t_type)
                .infix
                .expect("Internal error: Unexpected missing infix operation!!");
            infix_rule(self, can_assign);
        }

        if can_assign && self.match_next(Type::Equal) {
            self.error_at_current("Invalid assignment target.");
            self.expression();
        }
    }

    fn resolve_local(&mut self, name: Token) -> Option<usize> {
        for (index, local) in self.locals.iter().enumerate().rev() {
            if name.lexeme == local.name.lexeme {
                if !local.initialized {
                    self.error_at_current("Cannot read variable in its own initializer.");
                }
                return Some(index);
            }
        }
        return None;
    }

    fn parse_variable(&mut self, message: &str) -> String {
        self.consume(Type::Identifier, message);

        self.declare_variable();
        if self.scope_depth > 0 {
            return String::from("");
        }

        self.identifier_constant(self.previous)
    }

    fn identifier_constant(&mut self, name: Token) -> String {
        name.lexeme.to_string()
    }

    fn declare_variable(&mut self) {
        if self.scope_depth == 0 {
            return;
        }

        let name = self.previous;

        // TODO: Oof
        let mut local_exists = false;
        for local in self.locals.iter().rev() {
            if local.depth < self.scope_depth {
                break;
            }
            if name.lexeme == local.name.lexeme {
                local_exists = true;
                break;
            }
        }
        if local_exists {
            self.error_at_current("Variable with same name already declared in this scope.");
        }

        self.add_local(name);
    }

    fn define_variable(&mut self, global: String) {
        if self.scope_depth > 0 {
            self.mark_initialized();
            return;
        }
        self.emit_opcode(OpCode::DefineGlobal(global));
    }

    fn add_local(&mut self, name: Token<'a>) {
        self.locals.push(Local {
            name,
            depth: self.scope_depth,
            initialized: false,
        });
    }

    fn mark_initialized(&mut self) {
        if self.scope_depth == 0 {
            return;
        }
        self.locals.last_mut().unwrap().initialized = true;
    }

    fn advance(&mut self) {
        self.previous = self.current;

        loop {
            self.current = self.scanner.scan_token();
            if let Type::Error = self.current.t_type {
                ()
            } else {
                break;
            } // TODO: Inverted if-let???

            self.error_at_current(self.current.lexeme);
        }
    }

    fn match_next(&mut self, t_type: Type) -> bool {
        if !self.check(t_type) {
            return false;
        }
        self.advance();
        true
    }

    fn consume(&mut self, t_type: Type, message: &str) {
        if t_type == self.current.t_type {
            self.advance();
        } else {
            self.error_at_current(message);
        }
    }

    fn check(&self, t_type: Type) -> bool {
        t_type == self.current.t_type
    }

    fn emit_opcode(&mut self, code: OpCode) {
        self.compiling_chunk.code.push(OpCodeLine {
            code,
            line: self.previous.line,
        })
    }

    fn emit_opcodes(&mut self, code1: OpCode, code2: OpCode) {
        self.emit_opcode(code1);
        self.emit_opcode(code2);
    }

    fn emit_jump(&mut self, code: OpCode) -> usize {
        self.emit_opcode(code);
        self.compiling_chunk.code.len() - 1
    }

    fn patch_jump(&mut self, offset: usize) {
        let jump = self.compiling_chunk.code.len() - offset - 1;

        self.compiling_chunk.code[offset].code = match self.compiling_chunk.code[offset].code {
            OpCode::Jump(_) => OpCode::Jump(jump),
            OpCode::JumpIfFalse(_) => OpCode::JumpIfFalse(jump),
            _ => panic!("Jump was tried to be patched, opcode was not a jump!"),
        }
    }

    fn emit_loop(&mut self, start: usize) {
        let jump = self.compiling_chunk.code.len() - start + 1;
        self.emit_opcode(OpCode::Loop(jump));
    }

    fn end_compiliation(&mut self) {
        self.emit_opcode(OpCode::Return);
    }

    fn begin_scope(&mut self) {
        self.scope_depth += 1;
    }

    fn end_scope(&mut self) {
        self.scope_depth -= 1;

        while self.locals.len() > 0
            && self.locals.last().expect("Empty locals?").depth > self.scope_depth
        {
            self.emit_opcode(OpCode::Pop);
            self.locals.pop();
        }
    }

    fn error_at_current(&mut self, message: &str) {
        self.error_at(self.current, message)
    }

    fn error_at(&mut self, token: Token, message: &str) {
        if self.panic_mode {
            return;
        }

        eprint!("[Line {}] Error", token.line);
        match token.t_type {
            Type::EOF => eprint!(" at end"),
            Type::Error => (),
            _ => eprint!(" at line {}", token.line),
        }
        eprintln!(": {}", message);

        self.had_error = true;
        self.panic_mode = true;
    }

    fn syncronize(&mut self) {
        self.panic_mode = false;

        while self.current.t_type != Type::EOF {
            if self.previous.t_type == Type::Semicolon {
                return;
            }

            match self.current.t_type {
                Type::Class
                | Type::Fun
                | Type::Var
                | Type::For
                | Type::If
                | Type::While
                | Type::Print
                | Type::Return => return,
                _ => (),
            }
        }

        self.advance();
    }

    fn get_rule(t_type: Type) -> &'static ParseRule {
        &RULES[t_type.to_usize()]
    }

    pub fn new(chunk: &'a mut Chunk) -> Compiler<'a> {
        // Note: All struct values are initialized to stub values.
        // TODO: Find a way to create a stubbed chunk reference without having to pass one in
        Compiler {
            scanner: Scanner::new(""),
            compiling_chunk: chunk,

            previous: Token {
                t_type: Type::Error,
                lexeme: "\0",
                line: 0,
            },
            current: Token {
                t_type: Type::Error,
                lexeme: "\0",
                line: 0,
            },

            had_error: false,
            panic_mode: false,

            locals: Vec::with_capacity(8),
            scope_depth: 0,
        }
    }
}

plain_enum_mod! {this, Precedence {
    None,
    Assignment,
    Or,
    And,
    Equality,
    Comparison,
    Term,
    Factor,
    Unary,
    Call,
    Primary,
}}

struct Local<'a> {
    name: Token<'a>,
    depth: usize,
    initialized: bool,
}

struct ParseRule {
    prefix: Option<fn(&mut Compiler, bool)>,
    infix: Option<fn(&mut Compiler, bool)>,
    precedence: Precedence,
}

impl ParseRule {
    const fn new(precedence: Precedence) -> ParseRule {
        ParseRule {
            prefix: None,
            infix: None,
            precedence,
        }
    }

    const fn new_both(
        prefix: fn(&mut Compiler, bool),
        infix: Option<fn(&mut Compiler, bool)>,
        precedence: Precedence,
    ) -> ParseRule {
        ParseRule {
            prefix: Some(prefix),
            infix,
            precedence,
        }
    }

    const fn new_infix(infix: fn(&mut Compiler, bool), precedence: Precedence) -> ParseRule {
        ParseRule {
            prefix: None,
            infix: Some(infix),
            precedence,
        }
    }
}

static RULES: [ParseRule; 40] = [
    ParseRule::new_both(|compiler, _| compiler.grouping(), None, Precedence::Call), // LEFT_PAREN
    ParseRule::new(Precedence::None),                                               // RIGHT_PAREN
    ParseRule::new(Precedence::None),                                               // LEFT_BRACE
    ParseRule::new(Precedence::None),                                               // RIGHT_BRACE
    ParseRule::new(Precedence::None),                                               // COMMA
    ParseRule::new(Precedence::Call),                                               // DOT
    ParseRule::new_both(
        |compiler, _| compiler.unary(),
        Some(|compiler, _| compiler.binary()),
        Precedence::Term,
    ), // MINUS
    ParseRule::new_infix(|compiler, _| compiler.binary(), Precedence::Term),        // PLUS
    ParseRule::new(Precedence::None),                                               // SEMICOLON
    ParseRule::new_infix(|compiler, _| compiler.binary(), Precedence::Factor),      // SLASH
    ParseRule::new_infix(|compiler, _| compiler.binary(), Precedence::Factor),      // STAR
    ParseRule::new_both(|compiler, _| compiler.unary(), None, Precedence::None),    // BANG
    ParseRule::new_infix(|compiler, _| compiler.binary(), Precedence::Equality),    // BANG_EQUAL
    ParseRule::new(Precedence::None),                                               // EQUAL
    ParseRule::new_infix(|compiler, _| compiler.binary(), Precedence::Equality),    // EQUAL_EQUAL
    ParseRule::new_infix(|compiler, _| compiler.binary(), Precedence::Comparison),  // GREATER
    ParseRule::new_infix(|compiler, _| compiler.binary(), Precedence::Comparison),  // GREATER_EQUAL
    ParseRule::new_infix(|compiler, _| compiler.binary(), Precedence::Comparison),  // LESS
    ParseRule::new_infix(|compiler, _| compiler.binary(), Precedence::Comparison),  // LESS_EQUAL
    ParseRule::new_both(
        |compiler, can_assign| compiler.variable(can_assign),
        None,
        Precedence::None,
    ), // IDENTIFIER
    ParseRule::new_both(|compiler, _| compiler.string(), None, Precedence::Term),   // STRING
    ParseRule::new_both(|compiler, _| compiler.literal(), None, Precedence::None),  // NUMBER
    ParseRule::new_infix(|compiler, _| compiler.and(), Precedence::And),            // AND
    ParseRule::new(Precedence::None),                                               // CLASS
    ParseRule::new(Precedence::None),                                               // ELSE
    ParseRule::new_both(|compiler, _| compiler.literal(), None, Precedence::None),  // FALSE
    ParseRule::new(Precedence::None),                                               // FOR
    ParseRule::new(Precedence::None),                                               // FUN
    ParseRule::new(Precedence::None),                                               // IF
    ParseRule::new_both(|compiler, _| compiler.literal(), None, Precedence::None),  // NIL
    ParseRule::new_infix(|compiler, _| compiler.or(), Precedence::Or),              // OR
    ParseRule::new(Precedence::None),                                               // PRINT
    ParseRule::new(Precedence::None),                                               // RETURN
    ParseRule::new(Precedence::None),                                               // SUPER
    ParseRule::new(Precedence::None),                                               // THIS
    ParseRule::new_both(|compiler, _| compiler.literal(), None, Precedence::None),  // TRUE
    ParseRule::new(Precedence::None),                                               // VAR
    ParseRule::new(Precedence::None),                                               // WHILE
    ParseRule::new(Precedence::None),                                               // ERROR
    ParseRule::new(Precedence::None),                                               // EOF
];
