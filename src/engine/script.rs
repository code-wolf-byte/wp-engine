pub struct ScriptEnv {
    pub runtime: f32,
    pub time_of_day: f32,
}

pub fn eval_update(script: &str, current_value: f32, env: &ScriptEnv) -> Option<f32> {
    let body = extract_update_body(script)?;
    let ret_expr = extract_return_expr(body)?;
    eval_expr(ret_expr.trim(), current_value, env)
}

pub fn eval_script_opt(script: Option<&str>, current_value: f32, env: &ScriptEnv) -> f32 {
    match script {
        Some(s) => eval_update(s, current_value, env).unwrap_or(current_value),
        None => current_value,
    }
}

pub fn eval_vec3_script_opt(animated: &crate::engine::model::AnimatedValue, env: &ScriptEnv) -> [f32; 3] {
    // Default to [1.0; 3] so scale and color don't zero-out the output when missing
    let base = animated.as_vec3().unwrap_or([1.0; 3]);
    match &animated.script {
        None => base,
        Some(script) => [
            eval_update(script, base[0], env).unwrap_or(base[0]),
            eval_update(script, base[1], env).unwrap_or(base[1]),
            eval_update(script, base[2], env).unwrap_or(base[2]),
        ],
    }
}

fn extract_update_body(src: &str) -> Option<&str> {
    let sig = src.find("function update(")?;
    let after_sig = &src[sig..];
    let open = after_sig.find('{')?;
    let body_start = sig + open + 1;
    // match braces
    let mut depth = 1usize;
    let bytes = src.as_bytes();
    let mut i = body_start;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    if depth == 0 {
        Some(&src[body_start..i - 1])
    } else {
        None
    }
}

fn extract_return_expr(body: &str) -> Option<&str> {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("return ") {
            return Some(rest.trim_end_matches(';').trim());
        }
    }
    None
}

// ── Lexer ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f32),
    Ident(String),
    Dot,
    LParen,
    RParen,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Lt,
    Gt,
    LtEq,
    GtEq,
    EqEq,
    BangEq,
    AmpAmp,
    PipePipe,
    Question,
    Colon,
    Comma,
    Eof,
}

fn lex(src: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' | '\n' | '\r' => { i += 1; }
            '(' => { tokens.push(Token::LParen); i += 1; }
            ')' => { tokens.push(Token::RParen); i += 1; }
            '+' => { tokens.push(Token::Plus); i += 1; }
            '-' => { tokens.push(Token::Minus); i += 1; }
            '*' => { tokens.push(Token::Star); i += 1; }
            '/' => { tokens.push(Token::Slash); i += 1; }
            '%' => { tokens.push(Token::Percent); i += 1; }
            ',' => { tokens.push(Token::Comma); i += 1; }
            '.' => { tokens.push(Token::Dot); i += 1; }
            '?' => { tokens.push(Token::Question); i += 1; }
            ':' => { tokens.push(Token::Colon); i += 1; }
            '<' => {
                if i + 1 < chars.len() && chars[i+1] == '=' {
                    tokens.push(Token::LtEq); i += 2;
                } else {
                    tokens.push(Token::Lt); i += 1;
                }
            }
            '>' => {
                if i + 1 < chars.len() && chars[i+1] == '=' {
                    tokens.push(Token::GtEq); i += 2;
                } else {
                    tokens.push(Token::Gt); i += 1;
                }
            }
            '=' if i + 1 < chars.len() && chars[i+1] == '=' => {
                tokens.push(Token::EqEq); i += 2;
            }
            '!' if i + 1 < chars.len() && chars[i+1] == '=' => {
                tokens.push(Token::BangEq); i += 2;
            }
            '&' if i + 1 < chars.len() && chars[i+1] == '&' => {
                tokens.push(Token::AmpAmp); i += 2;
            }
            '|' if i + 1 < chars.len() && chars[i+1] == '|' => {
                tokens.push(Token::PipePipe); i += 2;
            }
            c if c.is_ascii_digit() || (c == '.' && i + 1 < chars.len() && chars[i+1].is_ascii_digit()) => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                tokens.push(Token::Num(s.parse().unwrap_or(0.0)));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                tokens.push(Token::Ident(s));
            }
            _ => { i += 1; }
        }
    }
    tokens.push(Token::Eof);
    tokens
}

// ── Parser ───────────────────────────────────────────────────────────────────

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    current_value: f32,
    env_runtime: f32,
    env_time_of_day: f32,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn consume(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() { self.pos += 1; }
        t
    }

    fn expect_lparen(&mut self) -> Option<()> {
        if *self.peek() == Token::LParen { self.consume(); Some(()) } else { None }
    }

    fn expect_comma(&mut self) -> Option<()> {
        if *self.peek() == Token::Comma { self.consume(); Some(()) } else { None }
    }

    fn expect_rparen(&mut self) -> Option<()> {
        if *self.peek() == Token::RParen { self.consume(); Some(()) } else { None }
    }

    // ternary: expr ? expr : expr
    fn parse_ternary(&mut self) -> Option<f32> {
        let cond = self.parse_or()?;
        if *self.peek() == Token::Question {
            self.consume();
            let a = self.parse_ternary()?;
            if *self.peek() != Token::Colon { return None; }
            self.consume();
            let b = self.parse_ternary()?;
            return Some(if cond != 0.0 { a } else { b });
        }
        Some(cond)
    }

    fn parse_or(&mut self) -> Option<f32> {
        let mut v = self.parse_and()?;
        while *self.peek() == Token::PipePipe {
            self.consume();
            let r = self.parse_and()?;
            v = if v != 0.0 || r != 0.0 { 1.0 } else { 0.0 };
        }
        Some(v)
    }

    fn parse_and(&mut self) -> Option<f32> {
        let mut v = self.parse_cmp()?;
        while *self.peek() == Token::AmpAmp {
            self.consume();
            let r = self.parse_cmp()?;
            v = if v != 0.0 && r != 0.0 { 1.0 } else { 0.0 };
        }
        Some(v)
    }

    fn parse_cmp(&mut self) -> Option<f32> {
        let mut v = self.parse_add()?;
        loop {
            let op = self.peek().clone();
            match op {
                Token::Lt    => { self.consume(); let r = self.parse_add()?; v = if v < r { 1.0 } else { 0.0 }; }
                Token::Gt    => { self.consume(); let r = self.parse_add()?; v = if v > r { 1.0 } else { 0.0 }; }
                Token::LtEq  => { self.consume(); let r = self.parse_add()?; v = if v <= r { 1.0 } else { 0.0 }; }
                Token::GtEq  => { self.consume(); let r = self.parse_add()?; v = if v >= r { 1.0 } else { 0.0 }; }
                Token::EqEq  => { self.consume(); let r = self.parse_add()?; v = if (v - r).abs() < 1e-7 { 1.0 } else { 0.0 }; }
                Token::BangEq => { self.consume(); let r = self.parse_add()?; v = if (v - r).abs() >= 1e-7 { 1.0 } else { 0.0 }; }
                _ => break,
            }
        }
        Some(v)
    }

    fn parse_add(&mut self) -> Option<f32> {
        let mut v = self.parse_mul()?;
        loop {
            match self.peek().clone() {
                Token::Plus  => { self.consume(); v += self.parse_mul()?; }
                Token::Minus => { self.consume(); v -= self.parse_mul()?; }
                _ => break,
            }
        }
        Some(v)
    }

    fn parse_mul(&mut self) -> Option<f32> {
        let mut v = self.parse_unary()?;
        loop {
            match self.peek().clone() {
                Token::Star    => { self.consume(); v *= self.parse_unary()?; }
                Token::Slash   => { self.consume(); let r = self.parse_unary()?; v /= if r == 0.0 { 1.0 } else { r }; }
                Token::Percent => { self.consume(); let r = self.parse_unary()?; v = v % if r == 0.0 { 1.0 } else { r }; }
                _ => break,
            }
        }
        Some(v)
    }

    fn parse_unary(&mut self) -> Option<f32> {
        if *self.peek() == Token::Minus {
            self.consume();
            return Some(-self.parse_primary()?);
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Option<f32> {
        match self.peek().clone() {
            Token::Num(n) => { self.consume(); Some(n) }
            Token::LParen => {
                self.consume();
                let v = self.parse_ternary()?;
                self.expect_rparen()?;
                Some(v)
            }
            Token::Ident(name) => {
                self.consume();
                // handle dotted names: engine.runtime, Math.sin, etc.
                let mut full = name.clone();
                while *self.peek() == Token::Dot {
                    self.consume();
                    if let Token::Ident(part) = self.peek().clone() {
                        self.consume();
                        full.push('.');
                        full.push_str(&part);
                    }
                }
                // variable?
                match full.as_str() {
                    "value" => return Some(self.current_value),
                    "engine.runtime" => return Some(self.env_runtime),
                    "engine.timeOfDay" => return Some(self.env_time_of_day),
                    "true" => return Some(1.0),
                    "false" => return Some(0.0),
                    _ => {}
                }
                // function call?
                if *self.peek() == Token::LParen {
                    self.consume();
                    let a = self.parse_ternary()?;
                    let result = match full.as_str() {
                        "Math.sin"   => { self.expect_rparen()?; a.sin() }
                        "Math.cos"   => { self.expect_rparen()?; a.cos() }
                        "Math.abs"   => { self.expect_rparen()?; a.abs() }
                        "Math.floor" => { self.expect_rparen()?; a.floor() }
                        "Math.ceil"  => { self.expect_rparen()?; a.ceil() }
                        "Math.round" => { self.expect_rparen()?; a.round() }
                        "Math.sqrt"  => { self.expect_rparen()?; a.sqrt() }
                        "Math.log"   => { self.expect_rparen()?; a.ln() }
                        "Math.pow" => {
                            self.expect_comma()?;
                            let b = self.parse_ternary()?;
                            self.expect_rparen()?;
                            a.powf(b)
                        }
                        "Math.min" => {
                            self.expect_comma()?;
                            let b = self.parse_ternary()?;
                            self.expect_rparen()?;
                            a.min(b)
                        }
                        "Math.max" => {
                            self.expect_comma()?;
                            let b = self.parse_ternary()?;
                            self.expect_rparen()?;
                            a.max(b)
                        }
                        "Math.atan2" => {
                            self.expect_comma()?;
                            let b = self.parse_ternary()?;
                            self.expect_rparen()?;
                            a.atan2(b)
                        }
                        "WEMath.smoothStep" => {
                            self.expect_comma()?;
                            let edge1 = self.parse_ternary()?;
                            self.expect_comma()?;
                            let x = self.parse_ternary()?;
                            self.expect_rparen()?;
                            // edge0=a, edge1=edge1, x=x
                            let t = ((x - a) / (edge1 - a)).clamp(0.0, 1.0);
                            t * t * (3.0 - 2.0 * t)
                        }
                        _ => { self.expect_rparen()?; 0.0 }
                    };
                    return Some(result);
                }
                None
            }
            _ => None,
        }
    }
}

fn eval_expr(src: &str, current_value: f32, env: &ScriptEnv) -> Option<f32> {
    let tokens = lex(src);
    let mut parser = Parser {
        tokens,
        pos: 0,
        current_value,
        env_runtime: env.runtime,
        env_time_of_day: env.time_of_day,
    };
    parser.parse_ternary()
}
