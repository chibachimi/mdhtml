use core::panic;
use std::{
    alloc::System,
    fs::File,
    io::{BufReader, Read, Write},
};

#[derive(Debug, PartialEq)]
enum TokenKind {
    Header(String, usize), // #~ The usize says how many # exist, which determines the kind of header.
    Break,                 // '  '
    HorizontalRule,        // *** or --- or ___
    Emphasis(String, usize), // String is the text, usize is the length. We decide what kind of emphasis based on the usize
    Ordered(Vec<String>, usize, usize), // 1., 2., ~. Usize values should be start line and end line.
    Unordered(Vec<String>, usize, usize),
    ListItem(String),
    Newline,                            // \n Will need this for <p>
    Paragraph(String, Vec<SpentToken>), // Contains a String of any length, will use the String later to add <p> and
    // </p> at the beginning and end.
    Code(String),
    Image(String, String, String), // Alt-text, Path, Title
    Link(String, String, String),
}

#[derive(Debug, PartialEq)]
struct SpentToken {
    token: TokenKind,
    col: usize,
    line: usize,
}

#[derive(Debug)]
struct Lexer {
    input: Vec<u8>,
    pos: usize,
    col: usize,
    line: usize,
}

impl Lexer {
    fn new(input: &str) -> Lexer {
        Lexer {
            input: input.as_bytes().to_vec(),
            pos: 0,
            col: 1,
            line: 1,
        }
    }

    fn current(&self) -> Option<u8> {
        if self.pos < self.input.len() {
            Some(self.input[self.pos])
        } else {
            None
        }
    }

    fn advance(&mut self) {
        if self.pos < self.input.len() {
            if self.input[self.pos] == b'\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            self.pos += 1;
        }
    }

    // Can be used to spot <br>
    fn peak(&self) -> Option<u8> {
        if self.pos + 1 < self.input.len() {
            Some(self.input[self.pos + 1])
        } else {
            None
        }
    }

    fn behind(&self) -> Option<u8> {
        if self.pos - 1 < self.input.len() {
            Some(self.input[self.pos - 1])
        } else {
            None
        }
    }

    fn tokenize(&mut self) -> Vec<SpentToken> {
        let mut tokens = Vec::new();

        while let Some(c) = self.current() {
            match c {
                // TODO Split off the whitespace to check for a <br>
                b'\t' | b'\r' => {
                    self.advance();
                }
                b'\n' => {
                    tokens.push(SpentToken {
                        token: TokenKind::Newline,
                        col: self.col,
                        line: self.line,
                    });
                    self.advance();
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => {
                    if self.is_ordered() {
                        let (text, col, line, end) = self.get_list();
                        tokens.push(SpentToken {
                            token: TokenKind::Ordered(text, line, end),
                            col,
                            line,
                        });
                    } else {
                        let (para, col, line, subtokens) = self.read_para();
                        tokens.push(SpentToken {
                            token: TokenKind::Paragraph(para, subtokens),
                            col,
                            line,
                        });
                        self.advance();
                    }
                }
                b' ' => match self.peak() {
                    Some(b' ') => {
                        tokens.push(SpentToken {
                            token: TokenKind::Break,
                            col: self.col,
                            line: self.line,
                        });
                        self.advance();
                    }
                    _ => self.advance(),
                },
                // Checks if unordered list or bold/bold italic.
                // For now, we hard check for formatting.
                // If a * is followed by whitespace, it is a list.
                // If not, it is bold/italic/bold italic.
                // Emphasis markers *are not* followed by whitespace *no matter what*
                // If it detects there is a horizontal rule, it will push that token then skip the
                // rest
                b'*' | b'+' | b'=' | b'-' => {
                    if self.peak() == Some(b' ') {
                        // unordered list
                        let (text, col, line, end) = self.get_list();
                        tokens.push(SpentToken {
                            token: TokenKind::Unordered(text, line, end),
                            line,
                            col,
                        });
                    } else if self.if_horizontal() {
                        let (col, line) = self.get_horizontal();
                        tokens.push(SpentToken {
                            token: TokenKind::HorizontalRule,
                            col,
                            line,
                        });
                    } else {
                        // emphasis
                        let (text, count, col, line) = self.get_emphasis();
                        tokens.push(SpentToken {
                            token: TokenKind::Emphasis(text, count),
                            col,
                            line,
                        });
                    }
                }
                b'#' => {
                    let (text, count, col, line) = self.get_header();
                    tokens.push(SpentToken {
                        token: TokenKind::Header(text, count),
                        col,
                        line,
                    });
                }
                b'`' => {
                    let (text, col, line) = self.get_code();
                    tokens.push(SpentToken {
                        token: TokenKind::Code(text),
                        col,
                        line,
                    });
                }
                b'!' => {
                    if self.is_image() {
                        self.advance();
                        let (alt, path, title, col, line) = self.get_linked_item();
                        tokens.push(SpentToken {
                            token: TokenKind::Image(alt, path, title),
                            col,
                            line,
                        });
                    } else {
                        continue;
                    }
                }
                b'[' => {
                    // TODO We probs need a way to check if this is a link or just text
                    let (text, link, title, col, line) = self.get_linked_item();
                    tokens.push(SpentToken {
                        token: TokenKind::Link(text, link, title),
                        col,
                        line,
                    });
                }
                // Default to just pushing anything else as a paragraph. Good idea? Maybe not.
                _ => {
                    let (para, col, line, subtokens) = self.read_para();
                    tokens.push(SpentToken {
                        token: TokenKind::Paragraph(para, subtokens),
                        col,
                        line,
                    });
                }
            }
        }

        tokens
    }

    fn read_para(&mut self) -> (String, usize, usize, Vec<SpentToken>) {
        let line_start = self.line;
        let col_start = self.col;

        let mut words = String::new();
        let mut subtokens = Vec::new();

        // If it's a paragraph, we just push until a new line.
        while let Some(c) = self.current() {
            match c {
                b'\n' => {
                    self.advance();
                    break;
                }
                // Detect certain tokens: link, emphasis
                // Emphasis
                // TODO Make sure this check works
                // If the character after the marker is a space, its probably just text, continue
                b'_' | b'*' => {
                    if self.peak() != Some(b' ') {
                        let (text, count, col, line) = self.get_emphasis();
                        subtokens.push(SpentToken {
                            token: TokenKind::Emphasis(text, count),
                            col,
                            line,
                        });
                    } else {
                        continue;
                    }
                }
                // Link
                // TODO Check this better. Maybe see if there's an open parenthesis after this?
                // When we parse the text will be included in the link tag
                b'[' => {
                    let (text, link, title, col, line) = self.get_linked_item();
                    subtokens.push(SpentToken {
                        token: TokenKind::Link(text, link, title),
                        col,
                        line,
                    });
                }
                _ => {
                    words.push(c as char);
                    self.advance();
                }
            }
        }

        (words, col_start, line_start, subtokens)
    }

    // if self.prev == ' ', break?
    fn get_emphasis(&mut self) -> (String, usize, usize, usize) {
        let mut star = String::new();
        let mut text = String::new();

        let start_col = self.col;
        let start_line = self.line;

        while let Some(e) = self.current() {
            match e {
                // TODO Peak here, if a ' ', break
                b'*' | b'_' => {
                    if self.peak() == Some(b' ') {
                        star.push(e as char);
                        self.advance();
                        break;
                    } else {
                        star.push(e as char);
                        self.advance();
                    }
                }
                b'\n' => break,
                _ => {
                    // Track markers, if * count is equal to stars.len(), we're good
                    // TODO New idea, if  star.len() isnt 1, 2, or 3, panic LMAO
                    //if self.peak() == Some(b'*') {
                    if self.current() == Some(b'*') {
                        self.advance();
                    } else {
                        text.push(e as char);
                        self.advance();
                    }
                }
            }
        }
        let count = star.len() / 2;

        // Please don't look at this!
        if count == 1 || count == 2 || count == 3 {
            (text, count, start_col, start_line)
        } else {
            panic!(
                "Error at line: {}, col: {}: mismatched emphasis declaration.",
                start_line, start_col
            );
        }
    }

    // Just gets the number of # to determine header type. We parse later.
    fn get_header(&mut self) -> (String, usize, usize, usize) {
        let line_start = self.line;
        let col_start = self.col;

        let mut hashes = String::new();
        let mut text = String::new();

        while let Some(h) = self.current() {
            match h {
                b'#' => {
                    hashes.push(h as char);
                    self.advance();
                }
                b'\n' => {
                    break;
                }
                _ => {
                    // TODO Replicate the check we do in emphasis
                    text.push(h as char);
                    self.advance();
                }
            }
        }
        let count = hashes.len();
        if count <= 6 {
            (text, count, col_start, line_start)
        } else {
            panic!("Header error in line: {}, col: {}", line_start, col_start);
        }
    }

    fn is_ordered(&self) -> bool {
        if self.peak() == Some(b'.') {
            true
        } else {
            false
        }
    }

    fn get_list_item(&mut self) -> String {
        // We don't care about the marker, just eat the text until an end of line, then return
        let mut text = String::new();

        while let Some(t) = self.current() {
            match t {
                b'+' | b'-' | b'*' => {
                    self.advance();
                }
                // If number indicates a list, skip, otherwise push to text.
                b'1'..=b'9' => {
                    if self.peak() == Some(b'.') {
                        self.advance();
                        self.advance();
                    } else {
                        text.push(t as char);
                        self.advance();
                    }
                }
                b'\n' => {
                    break;
                }
                _ => {
                    text.push(t as char);
                    self.advance();
                }
            }
        }

        text
    }

    // TODO Something is fucked with lists
    fn get_list(&mut self) -> (Vec<String>, usize, usize, usize) {
        let start_line = self.line;
        let start_col = self.col;
        let mut end_line = self.line;

        let mut items: Vec<String> = Vec::new();

        while let Some(l) = self.current() {
            match l {
                b'*' | b'+' | b'-' => {
                    let item = self.get_list_item();
                    items.push(item);
                    self.advance();
                }
                b'1'..=b'9' => {
                    // Skip num and dot. They are not important.
                    // TODO Could include this in the get_list_item() method
                    //self.advance();
                    //self.advance();
                    let item = self.get_list_item();
                    items.push(item);
                    self.advance();
                }
                /* TODO Later
                 * b'\t' => {
                 * get_sub_list();
                 * }
                 */
                b'\n' | b'\r' => self.advance(),
                //b'\n' => {
                //    //match self.peak() {
                //    //    Some(b'\n') | Some(b' ') => break,
                //    //    _ => self.advance(),
                //    //}

                //    if self.peak() == Some(b'\n') {
                //        break;
                //    } else {
                //        self.advance();
                //    }

                //    //if self.peak() != Some(b'\n') {
                //    //    self.advance();
                //    //} else {
                //    //    break;
                //    //}
                //}
                _ => {
                    break;
                }
            }
        }

        (items, start_col, start_line, end_line)
    }

    // TODO Needs a lot more checking (length, alone on line, ect)
    // Checks if current character + 3 is a space (trying to enforce markdown syntax)
    // Then matches the current character + 2 is a hr character
    fn if_horizontal(&self) -> bool {
        // I think this is better than if statements
        //match self.input[self.pos + 3] {
        //    b' ' | b'\n' | b'-' => match self.input[self.pos + 2] {
        //        b'*' | b'-' | b'_' => true,
        //        _ => false,
        //    },
        //    _ => false,
        //}

        let mut pos = self.pos;
        let mut is_rule = false;

        while let i = self.input[pos] {
            match i {
                b'-' => {
                    pos += 1;
                    is_rule = true;
                }
                b' ' | b'\n' => {
                    pos += 1;
                    is_rule = true;
                    break;
                }
                _ => {
                    is_rule = false;
                    break;
                }
            }
        }
        is_rule

        //match self.input[self.pos + 2] {
        //    b'-' => true,
        //    b' ' | b'\n' => true,
        //    _ => false,
        //}

        //if self.input[self.pos + 3] == b' ' {
        //    match self.input[self.pos + 2] {
        //        b'+' | b'-' | b'_' => true,
        //        _ => false,
        //    }
        //} else {
        //    false
        //}
    }

    fn get_horizontal(&mut self) -> (usize, usize) {
        let col = self.col;
        let line = self.line;

        // skip rest of line. Enforce that horizontals should be *alone* on the line
        self.pos += 2;
        self.advance();

        (col, line)
    }

    fn get_code(&mut self) -> (String, usize, usize) {
        let col = self.col;
        let line = self.line;

        let mut text = String::new();

        while let Some(c) = self.peak() {
            match c {
                b'`' => {
                    // Skip the second backtick then return
                    self.advance();
                    self.advance();
                    break;
                }
                _ => {
                    text.push(c as char);
                    self.advance();
                }
            }
        }
        (text, col, line)
    }

    fn is_image(&self) -> bool {
        if self.peak() == Some(b'[') {
            true
        } else {
            false
        }
    }

    fn get_linked_item(&mut self) -> (String, String, String, usize, usize) {
        let col = self.col;
        let line = self.line;

        let mut alt = false;
        let mut li = false;
        let mut ti = false;

        let mut alt_text = String::new();
        let mut link = String::new();
        let mut title = String::new();

        while let Some(i) = self.current() {
            match i {
                b'[' => {
                    // eat the bracket.
                    self.advance();
                    // flip alt to true so we push everything to alt_text.
                    alt = true;
                }
                b']' => {
                    // opposite of open bracket.
                    self.advance();
                    alt = false;
                }
                b'(' => {
                    self.advance();
                    li = true;
                }
                b')' => {
                    // closed para is the end. eat, break, return tuple.
                    self.advance();
                    break;
                }
                b'"' => {
                    // If false, turn true so we can push to title
                    // if true, we've reached the end, so turn false then continue
                    li = false;
                    if !ti {
                        ti = true;
                        title.push(i as char);
                        self.advance();
                    } else {
                        ti = false;
                        title.push(i as char);
                        self.advance();
                    }
                }
                _ => {
                    if alt {
                        alt_text.push(i as char);
                        self.advance();
                    } else if li {
                        link.push(i as char);
                        self.advance();
                        // TODO title is included in the link, separated by quotes. They keep
                        // combining and not sure why.
                    } else if ti {
                        title.push(i as char);
                        self.advance();
                    } else {
                        panic!("Unidentified character! {}", i as char);
                    }
                }
            }
        }

        (alt_text, link, title, col, line)
    }
}

// TODO Could move a lot of these methods to independant functions
#[derive(Debug)]
struct Parser {
    tokens: Vec<SpentToken>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<SpentToken>) -> Parser {
        Parser { tokens, pos: 0 }
    }

    fn current(&self) -> Option<&SpentToken> {
        if self.pos < self.tokens.len() {
            Some(&self.tokens[self.pos])
        } else {
            None
        }
    }

    fn advance(&mut self) {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
    }

    //fn parse_tokens(&mut self) -> String {
    //    // match the current token, for each token use a different method for each type
    //    todo!()
    //}

    fn preview(&self) {
        // TODO Preivew html in terminal to choose if it's what we want.
        todo!()
    }

    fn parse_tokens(&mut self) -> Vec<String> {
        let mut parsed_text: Vec<String> = Vec::new();

        while let Some(t) = self.current() {
            match &t.token {
                TokenKind::Header(h, c) => {
                    let header = self.new_header(h, c);
                    parsed_text.push(header);
                    self.advance();
                }
                TokenKind::Break => {
                    let br = self.new_break();
                    parsed_text.push(br);
                    self.advance();
                }
                TokenKind::Unordered(list, _, _) => {
                    let unordered = self.new_unordered(list);
                    parsed_text.push(unordered);
                    self.advance();
                }
                TokenKind::Emphasis(e, c) => {
                    let em = self.new_emphasis(e, &c);
                    parsed_text.push(em);
                    self.advance();
                }
                TokenKind::Newline => {
                    let newline = self.new_line();
                    parsed_text.push(newline);
                    self.advance();
                }
                TokenKind::Paragraph(p, s) => {
                    let mut para = self.new_para(p);
                    if !s.is_empty() {
                        // If scary subtokens
                        s.iter().for_each(|item| {
                            match &item.token {
                                TokenKind::Emphasis(substring, subcount) => {
                                    let loc = &item.col;
                                    let em = self.new_emphasis(substring.as_str(), &subcount);
                                    // I'm not entirely sure why we need the + 2. I think for the
                                    // tag opener (<) and any space beforehand? Interesting.
                                    para.insert_str(*loc + 2, &em);
                                }
                                TokenKind::Link(subtext, subpath, subtitle) => {
                                    let loc = &item.col;
                                    let sublink = self.new_link(&subtext, &subpath, &subtitle);
                                    para.insert_str(*loc + 2, &sublink);
                                }
                                _ => {
                                    panic!("Unimplemented subtoken in paragraph");
                                }
                            }
                        });
                    }
                    parsed_text.push(para);
                    self.advance();
                    //let paragraph = self.new_para(p);
                    // TODO Should break apart this nest
                    //if self.is_actually_link(&p) {
                    //    let (text, path, title) = self.para_to_link(&p);
                    //    let link = self.new_link(text.as_str(), path.as_str(), title.as_str());
                    //    parsed_text.push(link);
                    //    self.advance();
                    //} else {
                    //    let mut paragraph = self.new_para(p);
                    //    if self.has_emphasis(&paragraph) {
                    //        self.add_emphasis(&mut paragraph);
                    //    }

                    //    parsed_text.push(paragraph);
                    //    self.advance();
                    //}

                    //parsed_text.push(paragraph);
                    //self.advance();
                }
                // TODO Start and End markers are probably useless. Remove.
                TokenKind::Ordered(list, s, e) => {
                    let ordered = self.new_ordered(list);
                    parsed_text.push(ordered);
                    self.advance();
                }
                TokenKind::ListItem(_) => {
                    // Throw an error bc these aren't supposed to be independant
                    panic!("Misplaced ListItem token found!");
                }
                TokenKind::HorizontalRule => {
                    let hor = self.new_horizontal();
                    parsed_text.push(hor);
                    self.advance();
                }
                TokenKind::Code(c) => {
                    let co = self.new_code(c);
                    parsed_text.push(co);
                    self.advance();
                }
                TokenKind::Image(alt, path, title) => {
                    let im = self.new_image(alt, path, title);
                    parsed_text.push(im);
                    self.advance();
                }
                TokenKind::Link(text, path, title) => {
                    let link = self.new_link(text, path, title);
                    parsed_text.push(link);
                    self.advance();
                }
            }
        }
        parsed_text
    }

    fn new_unordered(&self, raw_list: &Vec<String>) -> String {
        let mut full_list = String::from("<ul>");

        raw_list.iter().for_each(|item| {
            let mut full_item = String::from("<li>");

            full_item.push_str(item.as_str());
            full_item.push_str("</li>");
            full_list.push_str(full_item.as_str());
        });

        full_list.push_str("</ul>");
        full_list
    }

    fn new_ordered(&self, raw_list: &Vec<String>) -> String {
        let mut full_list = String::from("<ol>");

        raw_list.iter().for_each(|item| {
            let mut full_item = String::from("<li>");
            full_item.push_str(item.as_str());
            full_item.push_str("</li>");
            full_list.push_str(full_item.as_str());
        });

        full_list.push_str("</ol>");
        full_list
    }

    fn new_para(&self, para: &str) -> String {
        let tag = ("<p>", "</p>");

        let mut full = String::new();

        full.push_str(tag.0);
        full.push_str(para);
        full.push_str(tag.1);
        full
    }

    fn is_actually_link(&self, para: &str) -> bool {
        let chars = para.as_bytes();
        if chars.contains(&b'[') && chars.contains(&b']') {
            true
        } else {
            false
        }
    }

    fn para_to_link(&self, para: &str) -> (String, String, String) {
        let chars = para.as_bytes();
        let mut pos = 0;

        let mut alt = false;
        let mut li = false;
        let mut ti = false;

        let mut alt_text = String::new();
        let mut link = String::new();
        let mut title = String::new();

        while let i = chars[pos] {
            match i {
                b'[' => {
                    // eat the bracket.
                    pos += 1;
                    // flip alt to true so we push everything to alt_text.
                    alt = true;
                }
                b']' => {
                    // opposite of open bracket.
                    pos += 1;
                    alt = false;
                }
                b'(' => {
                    pos += 1;
                    li = true;
                }
                b')' => {
                    // closed para is the end. eat, break, return tuple.
                    pos += 1;
                    break;
                }
                b'"' => {
                    // If false, turn true so we can push to title
                    // if true, we've reached the end, so turn false then continue
                    li = false;
                    if !ti {
                        ti = true;
                        title.push(i as char);
                        pos += 1;
                    } else {
                        ti = false;
                        title.push(i as char);
                        pos += 1;
                    }
                }
                _ => {
                    if !li && !ti {
                        alt = true;
                        alt_text.push(i as char);
                        pos += 1;
                    } else if !alt && !ti {
                        li = true;
                        link.push(i as char);
                        pos += 1;
                    } else if !li && !alt {
                        ti = true;
                        title.push(i as char);
                        pos += 1;
                    } else {
                        panic!("Unidentified character {} in para_to_link()", i as char);
                    }
                }
            }
        }

        (alt_text, link, title)
    }

    // TODO Need to check if a paragraph has emphasis
    // TODO Needs better checking
    // If so, find where it is and add emph markers
    //fn has_emphasis(&self, para: &str) -> bool {
    //    let chars = para.as_bytes();
    //    if chars.contains(&b'*') | chars.contains(&b'-') | chars.contains(&b'+') {
    //        true
    //    } else {
    //        false
    //    }
    //}

    // TODO Scrap this?
    //fn add_emphasis(&self, para: &mut String) {
    //    // turn para into a vector of bytes
    //    // find emphasis
    //    // insert emphasis markers
    //    let chars = para.as_bytes();
    //    let mut pos = 0;
    //    // get indx
    //    // check what kind of emphasis
    //    para.insert_str(idx, string);
    //}

    fn new_header(&self, head: &str, count: &usize) -> String {
        let tag = match count {
            1 => ("<h1>", "</h1>"),
            2 => ("<h2>", "</h2>"),
            3 => ("<h3>", "</h3>"),
            4 => ("<h4>", "</h4>"),
            5 => ("<h5>", "</h5>"),
            6 => ("<h6>", "</h6>"),
            _ => panic!(),
        };
        let mut full = String::new();

        full.push_str(tag.0);
        full.push_str(head);
        full.push_str(tag.1);
        full
    }

    fn new_emphasis(&self, emphasis: &str, count: &usize) -> String {
        let tag = match count {
            1 => ("<em>", "</em>"),                  // italic
            2 => ("<strong>", "</strong>"),          // bold
            3 => ("<em><strong>", "</strong></em>"), // bold italic
            _ => panic!(),
        };

        let mut full = String::new();

        full.push_str(tag.0);
        full.push_str(emphasis);
        full.push_str(tag.1);
        full
    }

    fn new_code(&self, code: &str) -> String {
        let tag = ("<code>", "</code>");

        let mut full = String::new();

        full.push_str(tag.0);
        full.push_str(code);
        full.push_str(tag.1);
        full
    }

    // TODO Is there a better way of doing this?
    // TODO This does produce the desired result, but has backslashes
    fn new_link(&self, text: &str, path: &str, title: &str) -> String {
        // <a href = "www.linkgoeshere.com" title = "title goes here"> link text </a>

        let link_tag = ("<a ", ">", "</a>");
        let href_tag = "href = ";
        let title_tag = "title = ";
        let quote_literal = "\"";

        let mut full = String::new();

        let full_path = format!("{} {} {}", quote_literal, path, quote_literal);

        full.push_str(link_tag.0);
        full.push_str(href_tag);
        full.push_str(&full_path);
        full.push_str(title_tag);
        full.push_str(title);
        full.push_str(link_tag.1);
        full.push_str(text);
        full.push_str(link_tag.2);
        full
    }

    // TODO Same issue as above
    // This could be taken care of when printing to file
    fn new_image(&self, alt: &str, path: &str, title: &str) -> String {
        let img_tag = ("<img ", ">");
        let src_tag = "src= ";
        let title_tag = "title= ";
        let alt_tag = "alt= ";
        let quote_literal = '"';

        // Push quotation to path
        let mut path_string = String::new();
        path_string.push(quote_literal);
        path_string.push_str(path);
        path_string.push(quote_literal);

        // Push quotation to title
        let mut title_string = String::new();
        title_string.push(quote_literal);
        title_string.push_str(title);
        title_string.push(quote_literal);

        // Push quotation to alt text
        let mut alt_string = String::new();
        alt_string.push(quote_literal);
        alt_string.push_str(alt);
        alt_string.push(quote_literal);

        // Combine everything
        let mut full = String::new();
        full.push_str(img_tag.0);
        full.push_str(src_tag);
        full.push_str(path_string.as_str());
        full.push_str(title_tag);
        full.push_str(title_string.as_str());
        full.push_str(alt_tag);
        full.push_str(alt_string.as_str());
        full.push_str(img_tag.1);
        full
    }

    fn new_line(&self) -> String {
        String::from("\n")
    }

    fn new_break(&self) -> String {
        String::from("<br>")
    }

    fn new_horizontal(&self) -> String {
        String::from("<hr>")
    }
}

// TODO Placeholder. Delete.
fn display_example_markdown() {
    let markdown = String::from(
        "`<p>hello world</p>` Hi mom\n Here is a link to [DuckDuckGo](www.example.com)",
    );

    let mut lex = Lexer::new(&markdown);
    let tokens = lex.tokenize();

    println!("{}", &markdown);
    println!("{:?}", &tokens);

    let mut parse = Parser::new(tokens);
    let finished = parse.parse_tokens();

    println!("{:?}", finished);
}

fn output_html_file(name: &str, input: String) -> std::io::Result<()> {
    let mut full = String::new();
    full.push_str(name);
    full.push_str(".html");

    let mut file = File::create(full)?;
    file.write_all(input.as_bytes())?;

    Ok(())
}

fn unroll_parser(finished: Vec<String>) -> String {
    let mut output = String::new();

    // Use a loop to map every item in finished to output
    finished.iter().for_each(|item| output.push_str(item));
    output
}

fn get_file_text(name: &str) -> String {
    let mut full = String::new();
    if !name.contains(".md") {
        full.push_str(name);
        full.push_str(".md");
    }
    let file = File::open(full).unwrap();
    let mut buff = BufReader::new(file);
    let mut contents = String::new();

    buff.read_to_string(&mut contents).unwrap();
    contents
}

fn tokenize(contents: &String) -> Vec<SpentToken> {
    let mut lex = Lexer::new(&contents);
    let tokens = lex.tokenize();
    tokens
}

fn parse(tokens: Vec<SpentToken>) -> Vec<String> {
    let mut parse = Parser::new(tokens);
    let finished = parse.parse_tokens();
    finished
}

fn help() {
    todo!()
}

// TODO Add arguments
fn main() -> std::io::Result<()> {
    //let args: Vec<String> = std::env::args().collect();
    //if args.len() < 2 {
    //    help();
    //    std::process::exit(1);
    //}
    // TODO If not provided an output name, the program should take the input name, and replace the
    // md with html.
    //let file = &args[1];
    //let out_name = &args[2];
    // Read file and put contents into String
    // TODO Change this to an arg
    //let name = "example";
    let name = "example";
    let contents = get_file_text(&name);
    // Lex
    let tokens = tokenize(&contents);
    // Parse
    let finished = parse(tokens);
    // Unroll to String
    let input = unroll_parser(finished);
    println!("{}", &input);
    // Output
    output_html_file(&name, input)?;
    Ok(())

    // TODO Paragraph subtoken testing. REMOVE.
    //let para_test = String::from("This is a paragraph with some ***emphasis***. More words");
    //let para_test = String::from("My favorite search engine is [Bing](https://bing.com). More text");
    //let para_test = String::from("I'm not very special!");
    //let mut lex = Lexer::new(&para_test);
    //let tokens = lex.tokenize();
    //println!("{:?}", &tokens);
    //let mut parse = Parser::new(tokens);
    //let para_parse = parse.parse_tokens();
    //println!("{:?}", para_parse);

    // TODO Then we can output into a file and place it somewhere somewhere
    //println!("{:?}", finished);
    //Ok(())
}
