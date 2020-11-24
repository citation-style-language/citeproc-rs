use crate::String;
use crate::output::micro_html::MicroNode;
use crate::IngestOptions;
#[cfg(test)]
use crate::output::FormatCmd;
#[cfg(test)]
use crate::LocalizedQuotes;
#[cfg(test)]
use pretty_assertions::assert_eq;

pub fn parse_quotes(mut original: Vec<MicroNode>, options: &IngestOptions) -> Vec<MicroNode> {
    let matcher = QuoteMatcher {
        original: &original,
        options: &options,
    };
    let inters: Vec<_> = matcher.intermediates().collect();
    stamp(inters.len(), inters.into_iter(), &mut original, options)
}

#[test]
fn test_parse_quotes() {
    use crate::output::FormatCmd;
    assert_eq!(
        parse_quotes(
            vec![MicroNode::Text("'hello'".into())],
            &IngestOptions::default_with_quotes(LocalizedQuotes::simple())
        ),
        vec![MicroNode::Quoted {
            is_inner: false,
            localized: LocalizedQuotes::simple(),
            children: vec![MicroNode::Text("hello".into()),]
        }]
    );
    let options = Default::default();
    assert_eq!(
        parse_quotes(
            MicroNode::parse("<i>'quotes in italics'</i>", &options),
            &options
        ),
        vec![MicroNode::Formatted(
            vec![MicroNode::Quoted {
                is_inner: false,
                localized: LocalizedQuotes::simple(),
                children: vec![MicroNode::Text("quotes in italics".into()),]
            }],
            FormatCmd::FontStyleItalic
        )]
    );
}

#[derive(Debug)]
enum Intermediate {
    Event(EventOwned),
    Index(usize),
}

#[derive(Debug)]
struct QuotedStack {
    dest: Vec<MicroNode>,
    stack: Vec<(SFQuoteKind, Vec<MicroNode>)>,
}

impl QuotedStack {
    fn with_capacity(n: usize) -> Self {
        QuotedStack {
            dest: Vec::with_capacity(n),
            stack: Vec::new(),
        }
    }
    fn mut_ref(&mut self) -> &mut Vec<MicroNode> {
        if let Some((_kind, top)) = self.stack.last_mut() {
            top
        } else {
            &mut self.dest
        }
    }
    fn push(&mut self, node: MicroNode) {
        self.mut_ref().push(node)
    }
    fn push_str(&mut self, txt: &str) {
        let dest = self.mut_ref();
        if let Some(MicroNode::Text(ref mut string)) = dest.last_mut() {
            string.push_str(txt);
        } else {
            dest.push(MicroNode::Text(txt.into()));
        }
    }
    fn push_string(&mut self, txt: String) {
        let dest = self.mut_ref();
        if let Some(MicroNode::Text(ref mut string)) = dest.last_mut() {
            string.push_str(&txt);
        } else {
            dest.push(MicroNode::Text(txt))
        }
    }
    fn collapse_hanging(mut self) -> Vec<MicroNode> {
        while let Some((kind, quoted)) = self.stack.pop() {
            self.push_str(kind.unmatched_str());
            for node in quoted {
                match node {
                    MicroNode::Text(txt) => self.push_string(txt),
                    _ => self.push(node),
                }
            }
        }
        self.dest
    }
}

fn stamp<'a>(
    len_hint: usize,
    intermediates: impl Iterator<Item = Intermediate>,
    orig: &mut Vec<MicroNode>,
    options: &IngestOptions,
) -> Vec<MicroNode> {
    let mut stack = QuotedStack::with_capacity(len_hint);
    let mut drained = 0;
    let drain = |start: usize,
                 end: usize,
                 drained: &mut usize,
                 orig: &mut Vec<MicroNode>,
                 stack: &mut QuotedStack| {
        stack
            .mut_ref()
            .extend(orig.drain(start - *drained..end - *drained));
        *drained += end - start;
    };
    let mut range_wip: Option<(usize, usize)> = None;
    for inter in intermediates {
        // NEXT: turn this into a struct so you can get mutable reference to dest without
        // having it hanging around in target, and don't need this if statement replicated
        // everywhere.
        match inter {
            Intermediate::Event(ev) => {
                if let Some(range) = range_wip {
                    drain(range.0, range.1, &mut drained, orig, &mut stack);
                    range_wip = None;
                }
                match ev {
                    EventOwned::Text(txt) => stack.push_string(txt),
                    EventOwned::SmartMidwordInvertedComma => stack.push_str("\u{2019}"),
                    EventOwned::SmartQuoteSingleOpen => {
                        stack.stack.push((SFQuoteKind::Single, Vec::new()));
                    }
                    EventOwned::SmartQuoteDoubleOpen => {
                        stack.stack.push((SFQuoteKind::Double, Vec::new()));
                    }
                    EventOwned::SmartQuoteSingleClose => {
                        if let Some((SFQuoteKind::Single, _)) = stack.stack.last() {
                            let (_, children) = stack.stack.pop().unwrap();
                            stack.push(MicroNode::Quoted {
                                is_inner: false,
                                localized: options.quotes.clone(),
                                children,
                            });
                        } else {
                            stack.push_str(SFQuoteKind::Single.unmatched_str());
                        }
                    }
                    EventOwned::SmartQuoteDoubleClose => {
                        if let Some((SFQuoteKind::Double, _)) = stack.stack.last() {
                            let (_, children) = stack.stack.pop().unwrap();
                            stack.push(MicroNode::Quoted {
                                is_inner: false,
                                localized: options.quotes.clone(),
                                children,
                            });
                        } else {
                            stack.push_str(SFQuoteKind::Double.unmatched_str());
                        }
                    }
                }
            }
            // Move sequential index references out of the array together where possible
            Intermediate::Index(ix) => {
                let node = orig.get_mut(ix - drained).unwrap();
                match node {
                    MicroNode::Quoted { children, .. }
                    | MicroNode::NoDecor(children)
                    | MicroNode::NoCase(children)
                    | MicroNode::Formatted(children, _) => {
                        let to_parse_owned = mem::replace(children, Vec::new());
                        let parsed = parse_quotes(to_parse_owned, options);
                        *children = parsed;
                    }
                    _ => {}
                };
                if let Some(ref mut range) = range_wip {
                    if range.1 == ix {
                        range.1 = ix + 1;
                    } else {
                        drain(range.0, range.1, &mut drained, orig, &mut stack);
                        range_wip = Some((ix, ix + 1));
                    }
                } else {
                    range_wip = Some((ix, ix + 1));
                }
            }
        }
    }
    if let Some(ref mut range) = range_wip {
        drain(range.0, range.1, &mut drained, orig, &mut stack);
    }
    stack.collapse_hanging()
}

#[test]
fn test_stamp() {
    // env_logger::init();
    let mut orig = vec![MicroNode::Text("hi".into()), MicroNode::Text("ho".into())];
    let options = IngestOptions::default_with_quotes(LocalizedQuotes::simple());
    let inters = vec![
        Intermediate::Event(EventOwned::Text("prefix, ".into())),
        Intermediate::Event(EventOwned::SmartQuoteSingleOpen),
        Intermediate::Index(0),
        Intermediate::Index(1),
        Intermediate::Event(EventOwned::Text("suffix".into())),
    ];
    assert_eq!(
        &stamp(2, inters.into_iter(), &mut orig, &options),
        &[MicroNode::Text("prefix, 'hihosuffix".into()),]
    );
    let mut orig = vec![MicroNode::Text("hi".into()), MicroNode::Text("ho".into())];
    let inters = vec![
        Intermediate::Event(EventOwned::Text("prefix, ".into())),
        Intermediate::Event(EventOwned::SmartQuoteSingleOpen),
        Intermediate::Index(0),
        Intermediate::Index(1),
        Intermediate::Event(EventOwned::SmartQuoteSingleClose),
        Intermediate::Event(EventOwned::Text(", suffix".into())),
    ];
    assert_eq!(
        &stamp(2, inters.into_iter(), &mut orig, &options),
        &[
            MicroNode::Text("prefix, ".into()),
            MicroNode::Quoted {
                localized: LocalizedQuotes::simple(),
                is_inner: false,
                children: vec![MicroNode::Text("hi".into()), MicroNode::Text("ho".into()),]
            },
            MicroNode::Text(", suffix".into()),
        ]
    );
}

#[derive(Debug)]
struct QuoteMatcher<'a> {
    original: &'a Vec<MicroNode>,
    options: &'a IngestOptions,
}

/// Find x in `[a, x]`, `[a, [b, [c, x]]]`, etc
fn leaning_text(node: &MicroNode, rightmost: bool) -> Option<&str> {
    match node {
        MicroNode::Quoted { ref children, .. }
        | MicroNode::NoDecor(ref children)
        | MicroNode::NoCase(ref children)
        | MicroNode::Formatted(ref children, _) => if rightmost {
            children.last()
        } else {
            children.first()
        }
        .and_then(|n| leaning_text(n, rightmost)),
        MicroNode::Text(text) => Some(text.as_str()),
    }
}

#[derive(Debug)]
enum EachSplitter<'a, I: Iterator<Item = Event<'a>> + 'a> {
    Index(Option<usize>),
    Splitter {
        splitter: I,
        seen_any: Option<bool>,
        index: usize,
    },
}

impl<'a, I: Iterator<Item = Event<'a>>> Iterator for EachSplitter<'a, I> {
    type Item = Intermediate;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            EachSplitter::Index(ref mut opt_ix) => {
                mem::replace(opt_ix, None).map(Intermediate::Index)
            }
            EachSplitter::Splitter {
                index,
                ref mut splitter,
                ref mut seen_any,
            } => {
                let nxt = splitter
                    .next()
                    .map(|ev| Intermediate::Event(ev.into()))
                    .or_else(|| {
                        mem::replace(seen_any, None).and_then(|any| {
                            if any {
                                None
                            } else {
                                Some(Intermediate::Index(*index))
                            }
                        })
                    });
                if nxt.is_some() {
                    *seen_any = Some(true);
                }
                nxt
            }
        }
    }
}

impl<'a> QuoteMatcher<'a> {
    fn intermediates(&'a self) -> impl Iterator<Item = Intermediate> + 'a {
        self.original
            .iter()
            .enumerate()
            .flat_map(move |(ix, node)| match node {
                MicroNode::Quoted { .. }
                | MicroNode::NoDecor(_)
                | MicroNode::NoCase(_)
                | MicroNode::Formatted(..) => EachSplitter::Index(Some(ix)),
                MicroNode::Text(ref string) => {
                    let prev = self
                        .original
                        .get(ix.wrapping_sub(1))
                        .and_then(|n| leaning_text(n, true));
                    let next = self
                        .original
                        .get(ix + 1)
                        .and_then(|n| leaning_text(n, false));
                    let splitter = QuoteSplitter::new(&string, prev, next).events();
                    EachSplitter::Splitter {
                        index: ix,
                        splitter,
                        seen_any: Some(false),
                    }
                }
            })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Event<'a> {
    Text(&'a str),
    SmartMidwordInvertedComma,
    SmartQuoteSingleOpen,
    SmartQuoteDoubleOpen,
    SmartQuoteSingleClose,
    SmartQuoteDoubleClose,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EventOwned {
    Text(String),
    SmartMidwordInvertedComma,
    SmartQuoteSingleOpen,
    SmartQuoteDoubleOpen,
    SmartQuoteSingleClose,
    SmartQuoteDoubleClose,
}

impl<'a> From<Event<'a>> for EventOwned {
    fn from(ev: Event<'a>) -> Self {
        match ev {
            Event::Text(s) => EventOwned::Text(s.into()),
            Event::SmartMidwordInvertedComma => EventOwned::SmartMidwordInvertedComma,
            Event::SmartQuoteSingleOpen => EventOwned::SmartQuoteSingleOpen,
            Event::SmartQuoteDoubleOpen => EventOwned::SmartQuoteDoubleOpen,
            Event::SmartQuoteSingleClose => EventOwned::SmartQuoteSingleClose,
            Event::SmartQuoteDoubleClose => EventOwned::SmartQuoteDoubleClose,
        }
    }
}

fn quote_event<'a>(ch: (SmartQuoteKind, char)) -> Option<Event<'a>> {
    let ev = match ch {
        (SmartQuoteKind::OpenSingle, _) => Event::SmartQuoteSingleOpen,
        (SmartQuoteKind::CloseSingle, _) => Event::SmartQuoteSingleClose,
        (SmartQuoteKind::OpenDouble, _) => Event::SmartQuoteDoubleOpen,
        (SmartQuoteKind::CloseDouble, _) => Event::SmartQuoteDoubleClose,
        (SmartQuoteKind::Open, '\'') => Event::SmartQuoteSingleOpen,
        (SmartQuoteKind::Close, '\'') => Event::SmartQuoteSingleClose,
        (SmartQuoteKind::Open, '"') => Event::SmartQuoteDoubleOpen,
        (SmartQuoteKind::Close, '"') => Event::SmartQuoteDoubleClose,
        (SmartQuoteKind::Midword, '\'') => Event::SmartMidwordInvertedComma,
        (SmartQuoteKind::Midword, '\u{2019}') => Event::SmartMidwordInvertedComma,
        // Don't parse this as a quote at all
        _ => {
            debug!("quote_event doesn't want a quote: {:?}", ch);
            return None;
        }
    };
    Some(ev)
}
use std::mem;
#[derive(Debug)]
struct Thingo<'a> {
    quote_event: Option<Event<'a>>,
    upto: Option<Event<'a>>,
    post: Option<Option<Event<'a>>>,
}
impl<'a> Thingo<'a> {
    fn post(s: &'a str) -> Self {
        Thingo {
            quote_event: None,
            upto: None,
            post: Some(Some(Event::Text(s))),
        }
    }
}
impl<'a> Iterator for Thingo<'a> {
    type Item = Event<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(ref mut post) = self.post {
            return mem::replace(post, None);
        }
        if self.quote_event.is_none() {
            return None;
        }
        mem::replace(&mut self.upto, None).or_else(|| mem::replace(&mut self.quote_event, None))
    }
}

impl<'a> Iterator for QuoteSplitter<'a> {
    type Item = Thingo<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some((ix, quote_char)) = self.possibles.next() {
            // next_char is either ' or "
            let mut prefix = &self.string[..ix];
            let mut suffix = &self.string[ix + quote_char.len_utf8()..];
            if prefix.is_empty() {
                if let Some(prev) = self.previous_text_node {
                    prefix = prev;
                }
            }
            if suffix.is_empty() {
                if let Some(next) = self.subsequent_text_node {
                    suffix = next;
                }
            }
            let upto = Some(Event::Text(&self.string[self.text_start..ix]));
            let quote_event = quote_kind(quote_char, prefix, suffix)
                .and_then(|kind| quote_event((kind, quote_char)));
            if quote_event.is_some() {
                self.text_start = ix + quote_char.len_utf8();
            }
            Some(Thingo {
                quote_event,
                upto,
                post: None,
            })
        } else if !self.emitted_last && self.text_start > 0 {
            // the remainder, after the last quote char
            self.emitted_last = true;
            Some(Thingo::post(&self.string[self.text_start..]))
        } else {
            None
        }
    }
}

fn quote_is_possible(ch: char) -> bool {
    match ch {
        SINGLE_OPEN | SINGLE_CLOSE | DOUBLE_OPEN | DOUBLE_CLOSE | '\'' | '"' => true,
        _ => false,
    }
}

#[derive(Debug)]
struct QuoteSplitter<'a> {
    string: &'a str,
    previous_text_node: Option<&'a str>,
    subsequent_text_node: Option<&'a str>,
    text_start: usize,
    possibles: std::iter::Filter<std::str::CharIndices<'a>, IsPossible>,
    emitted_last: bool,
}

type IsPossible = fn(c: &(usize, char)) -> bool;

impl<'a> QuoteSplitter<'a> {
    fn new(string: &'a str, prev: Option<&'a str>, next: Option<&'a str>) -> Self {
        QuoteSplitter {
            string,
            previous_text_node: prev,
            subsequent_text_node: next,
            text_start: 0,
            possibles: string
                .char_indices()
                .filter(|&(_, ch)| quote_is_possible(ch)),
            emitted_last: false,
        }
    }

    fn events(self) -> impl Iterator<Item = Event<'a>> {
        self.flat_map(|x| x).filter(|ev| match ev {
            Event::Text("") => false,
            _ => true,
        })
    }
}

#[test]
fn test_quote_splitter_simple() {
    let string = "hello, I'm a man with a plan, \"Canal Panama\".";
    let splitter = QuoteSplitter::new(string, None, None);
    let mut events = Vec::new();
    for event in splitter.events() {
        events.push(event);
    }
    assert_eq!(
        events,
        vec![
            Event::Text("hello, I"),
            Event::SmartMidwordInvertedComma,
            Event::Text("m a man with a plan, "),
            Event::SmartQuoteDoubleOpen,
            Event::Text("Canal Panama"),
            Event::SmartQuoteDoubleClose,
            Event::Text("."),
        ]
    );
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum SmartQuoteKind {
    Open,
    Close,
    Midword,
    OpenSingle,
    OpenDouble,
    CloseSingle,
    CloseDouble,
}

impl SmartQuoteKind {
    fn from_curly(ch: char) -> Option<Self> {
        if ch == SINGLE_OPEN {
            Some(SmartQuoteKind::OpenSingle)
        } else if ch == SINGLE_CLOSE {
            Some(SmartQuoteKind::CloseSingle)
        } else if ch == DOUBLE_OPEN {
            Some(SmartQuoteKind::OpenDouble)
        } else if ch == DOUBLE_CLOSE {
            Some(SmartQuoteKind::CloseDouble)
        } else {
            None
        }
    }
}

use super::puncttable::is_punctuation;

/// Determines what kind a smart quote should open at this point
fn quote_kind(character: char, prefix: &str, suffix: &str) -> Option<SmartQuoteKind> {
    let not_italic_ish = |c: &char| *c != '*' && *c != '~' && *c != '_' && *c != '\'' && *c != '"';

    // Beginning and end of line == whitespace.
    let next_char = suffix.chars().filter(not_italic_ish).nth(0).unwrap_or(' ');
    let prev_char = prefix
        .chars()
        .rev()
        .filter(not_italic_ish)
        .nth(0)
        .unwrap_or(' ');

    let next_white = next_char.is_whitespace();
    let prev_white = prev_char.is_whitespace();
    // i.e. braces and the like
    let not_term_punc = |c: char| is_punctuation(c) && c != '.' && c != ',';
    let wordy = |c: char| !is_punctuation(c) && !c.is_whitespace() && !c.is_control();

    let curly = SmartQuoteKind::from_curly(character);
    if let Some(curly) = curly {
        if let SmartQuoteKind::CloseSingle = curly {
            if wordy(prev_char) && wordy(next_char) {
                return Some(SmartQuoteKind::Midword);
            }
        }
        return Some(curly);
    } else if prev_white && next_white {
        None
    } else if prev_white && next_char.is_numeric() && character == '\'' {
        // '09 -- force a close quote
        Some(SmartQuoteKind::Midword)
    } else if !prev_white && next_white {
        Some(SmartQuoteKind::Close)
    } else if prev_white && !next_white {
        Some(SmartQuoteKind::Open)
    } else if next_white && (prev_char == '.' || prev_char == ',' || prev_char == '!') {
        Some(SmartQuoteKind::Close)
    } else if is_punctuation(prev_char) && not_term_punc(next_char) {
        Some(SmartQuoteKind::Close)
    } else if not_term_punc(prev_char) && is_punctuation(next_char) {
        Some(SmartQuoteKind::Open)
    } else if wordy(prev_char) && wordy(next_char) && character == '\'' {
        Some(SmartQuoteKind::Midword)
    } else if is_punctuation(prev_char) && wordy(next_char) {
        Some(SmartQuoteKind::Open)
    } else if wordy(prev_char) && is_punctuation(next_char) {
        Some(SmartQuoteKind::Close)
    } else {
        None
    }
}

#[derive(Debug)]
enum SFQuoteKind {
    Single,
    Double,
    // no midword
}

impl SFQuoteKind {
    fn unmatched_str(&self) -> &'static str {
        match self {
            SFQuoteKind::Single => "'",
            SFQuoteKind::Double => "\"",
        }
    }
}

const SINGLE_OPEN: char = '\u{2018}';
const SINGLE_CLOSE: char = '\u{2019}';
const DOUBLE_OPEN: char = '\u{201c}';
const DOUBLE_CLOSE: char = '\u{201d}';
