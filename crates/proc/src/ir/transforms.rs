use crate::disamb::names::NameIR;
use crate::names::NameToken;
use crate::prelude::*;
use citeproc_io::Cite;
use csl::Atom;
use std::mem;
use std::sync::{Arc, Mutex};

/////////////////////////////////
// capitalize start of cluster //
/////////////////////////////////

impl<O: OutputFormat> IR<O> {
    pub fn capitalize_first_term_of_cluster(&mut self, fmt: &O) {
        if let Some(trf) = self.find_term_rendered_first() {
            fmt.apply_text_case(trf, &IngestOptions {
                text_case: TextCase::CapitalizeFirst,
                ..Default::default()
            });
        }
    }
    // Gotta find a a CiteEdgeData::Term/LocatorLabel/FrnnLabel
    // (the latter two are also terms, but a different kind for disambiguation).
    fn find_term_rendered_first(&mut self) -> Option<&mut O::Build> {
        match self {
            IR::Rendered(Some(CiteEdgeData::Term(b))) |
            IR::Rendered(Some(CiteEdgeData::LocatorLabel(b))) |
            IR::Rendered(Some(CiteEdgeData::FrnnLabel(b))) => Some(b),
            // IR::ConditionalDisamb(c) => {
            //     let mut lock = c.lock().unwrap();
            //     lock.ir.find_term_rendered_first()
            // }
            IR::Seq(seq) => {
                // Search backwards because it's likely to be near the end
                seq.contents
                    .first_mut()
                    .and_then(|(ir, _)| ir.find_term_rendered_first())
            }
            _ => None,
        }
    }

}

////////////////////////
// second-field-align //
////////////////////////

impl<O: OutputFormat> IR<O> {
    pub fn split_first_field(&mut self) {
        // Pull off the first field of self -> [first, ...rest]
        if let Some(((first, gv), mut rest)) = match self {
            IR::Seq(seq) => if seq.contents.len() > 1 {
                Some(seq.contents.remove(0))
            } else {
                None
            }
            .and_then(|f| Some((f, mem::take(seq)))),
            _ => None,
        } {
            rest.display = Some(DisplayMode::RightInline);

            // Split the affixes into two sets with empty inside.
            let (afpre, afsuf) = rest
                .affixes
                .map(|mine| {
                    (
                        Some(Affixes {
                            prefix: mine.prefix,
                            suffix: Atom::from(""),
                        }),
                        Some(Affixes {
                            prefix: Atom::from(""),
                            suffix: mine.suffix,
                        }),
                    )
                })
                .unwrap_or((None, None));

            // Replace with joined splits
            *self = IR::Seq(IrSeq {
                contents: vec![
                    (
                        IR::Seq(IrSeq {
                            contents: vec![(first, gv)],
                            display: Some(DisplayMode::LeftMargin),
                            affixes: afpre,
                            ..Default::default()
                        }),
                        gv,
                    ),
                    (
                        IR::Seq(IrSeq {
                            contents: rest.contents,
                            display: Some(DisplayMode::RightInline),
                            affixes: afsuf,
                            ..Default::default()
                        }),
                        GroupVars::Important,
                    ),
                ],
                display: None,
                formatting: rest.formatting,
                affixes: None,
                delimiter: rest.delimiter.clone(),
                dropped_gv: None,
                quotes: rest.quotes.clone(),
                text_case: rest.text_case,
            });
        }
    }
}

////////////////////////////////
// Cite Grouping & Collapsing //
////////////////////////////////

impl<O: OutputFormat> IR<O> {
    pub fn first_name_block(&self) -> Option<Arc<Mutex<NameIR<O>>>> {
        match self {
            IR::Name(ref nir) => Some(nir.clone()),
            IR::ConditionalDisamb(c) => {
                let lock = c.lock().unwrap();
                lock.ir.first_name_block()
            }
            IR::Seq(seq) => {
                // assumes it's the first one that appears
                seq.contents.iter().find_map(|ir| ir.0.first_name_block())
            }
            _ => None,
        }
    }

    fn find_locator(&self) -> bool {
        match self {
            IR::Rendered(Some(CiteEdgeData::Locator(_))) => true,
            IR::ConditionalDisamb(c) => {
                let mut lock = c.lock().unwrap();
                lock.ir.find_locator()
            }
            IR::Seq(seq) => {
                // Search backwards because it's likely to be near the end
                seq.contents
                    .iter()
                    .rfind(|(ir, _)| ir.find_locator())
                    .is_some()
            }
            _ => false,
        }
    }

    fn find_first_year(&self) -> Option<O::Build> {
        match self {
            IR::Rendered(Some(CiteEdgeData::Year(b))) => Some(b.clone()),
            IR::ConditionalDisamb(c) => {
                let mut lock = c.lock().unwrap();
                lock.ir.find_first_year()
            }
            IR::Seq(seq) => seq.contents.iter().find_map(|(ir, _)| ir.find_first_year()),
            _ => None,
        }
    }

    fn find_first_year_and_suffix(&self) -> Option<(O::Build, u32)> {
        if let Some(fy) = self.find_first_year() {
            debug!("fy, {:?}", fy);
        }
        if let Some(ys) = self.find_year_suffix() {
            debug!("ys, {:?}", ys);
        }
        Some((self.find_first_year()?, self.find_year_suffix()?))
    }

    /// Rest of the name: "if it has a year suffix"
    fn suppress_first_year(&mut self, has_explicit: bool) -> bool {
        match self {
            IR::Rendered(opt @ Some(CiteEdgeData::Year(_))) => {
                *opt = None;
                true
            }
            IR::ConditionalDisamb(c) => {
                let mut lock = c.lock().unwrap();
                lock.ir.suppress_first_year(has_explicit);
                false
            }
            IR::Seq(seq) => {
                let mut found = if seq.contents.len() == 2 {
                    if let ((first, _), (second, gv)) = pair_at_mut(&mut seq.contents, 0).unwrap() {
                        match (second, gv) {
                            (IR::YearSuffix(_), GroupVars::Unresolved) if has_explicit => {
                                first.suppress_first_year(has_explicit)
                            }
                            (IR::YearSuffix(ys), GroupVars::Important)
                                if !has_explicit && !ys.ir.is_empty() =>
                            {
                                first.suppress_first_year(has_explicit)
                            }
                            _ => false,
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !found {
                    for (ir, _) in seq.contents.iter_mut() {
                        if ir.suppress_first_year(has_explicit) {
                            found = true;
                            break;
                        }
                    }
                }
                found
            }
            _ => false,
        }
    }

    pub fn find_year_suffix(&self) -> Option<u32> {
        self.has_implicit_year_suffix()
            .or_else(|| self.has_explicit_year_suffix())
    }

    pub fn has_implicit_year_suffix(&self) -> Option<u32> {
        match self {
            IR::YearSuffix(YearSuffix {
                hook: YearSuffixHook::Plain,
                ir,
                suffix_num: Some(n),
                ..
            }) if !ir.is_empty() => Some(*n),
            IR::ConditionalDisamb(c) => {
                let lock = c.lock().unwrap();
                lock.ir.has_implicit_year_suffix()
            }
            IR::Seq(seq) => {
                // assumes it's the first one that appears
                seq.contents
                    .iter()
                    .find_map(|ir| ir.0.has_implicit_year_suffix())
            }
            _ => None,
        }
    }

    pub fn has_explicit_year_suffix(&self) -> Option<u32> {
        match self {
            IR::YearSuffix(YearSuffix {
                hook: YearSuffixHook::Explicit(_),
                ir,
                suffix_num: Some(n),
                ..
            }) if !ir.is_empty() => Some(*n),
            IR::ConditionalDisamb(c) => {
                let lock = c.lock().unwrap();
                lock.ir.has_explicit_year_suffix()
            }
            IR::Seq(seq) => {
                // assumes it's the first one that appears
                seq.contents
                    .iter()
                    .find_map(|ir| ir.0.has_explicit_year_suffix())
            }
            _ => None,
        }
    }

    pub fn suppress_names(&self) {
        if let Some(fnb) = self.first_name_block() {
            let mut guard = fnb.lock().unwrap();
            *guard.ir = IR::Rendered(None);
        }
    }
    pub fn suppress_year(&mut self) {
        let has_explicit = self.has_explicit_year_suffix().is_some();
        if !has_explicit && self.has_implicit_year_suffix().is_none() {
            return;
        }
        self.suppress_first_year(has_explicit);
    }
}

impl<O: OutputFormat<Output = String>> IR<O> {
    pub fn collapse_to_cnum(&self, fmt: &O) -> Option<u32> {
        match self {
            IR::Rendered(Some(CiteEdgeData::CitationNumber(build))) => {
                // TODO: just get it from the database
                fmt.output(build.clone(), false).parse().ok()
            }
            IR::ConditionalDisamb(c) => {
                let lock = c.lock().unwrap();
                lock.ir.collapse_to_cnum(fmt)
            }
            IR::Seq(seq) => {
                // assumes it's the first one that appears
                if seq.contents.len() != 1 {
                    None
                } else {
                    seq.contents
                        .first()
                        .and_then(|(x, _)| x.collapse_to_cnum(fmt))
                }
            }
            _ => None,
        }
    }
}

use crate::db::IrGen;
use csl::Collapse;
use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct CnumIx {
    pub cnum: u32,
    pub ix: usize,
    pub force_single: bool,
}

impl CnumIx {
    fn new(c: u32, ix: usize) -> Self {
        CnumIx {
            cnum: c,
            ix,
            force_single: false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RangePiece {
    /// If the length of the range is only two, it should be rendered with a comma anyway
    Range(CnumIx, CnumIx),
    Single(CnumIx),
}

impl RangePiece {
    /// Return value is the previous value, to be emitted, if the next it couldn't be appended
    fn attempt_append(&mut self, nxt: CnumIx) -> Option<RangePiece> {
        *self = match self {
            _ if nxt.force_single => return Some(std::mem::replace(self, RangePiece::Single(nxt))),
            RangePiece::Single(prv) if prv.cnum == nxt.cnum - 1 => RangePiece::Range(*prv, nxt),
            RangePiece::Range(_, end) if end.cnum == nxt.cnum - 1 => {
                *end = nxt;
                return None;
            }
            _ => return Some(std::mem::replace(self, RangePiece::Single(nxt))),
        };
        return None;
    }
}

#[test]
fn range_append() {
    let mut range = RangePiece::Single(CnumIx::new(1, 1));
    let emit = range.attempt_append(CnumIx::new(2, 2));
    assert_eq!(
        (range, emit),
        (
            RangePiece::Range(CnumIx::new(1, 1), CnumIx::new(2, 2)),
            None
        )
    );
    let mut range = RangePiece::Single(CnumIx::new(1, 1));
    let emit = range.attempt_append(CnumIx::new(3, 2));
    assert_eq!(
        (range, emit),
        (
            RangePiece::Single(CnumIx::new(3, 2)),
            Some(RangePiece::Single(CnumIx::new(1, 1)))
        )
    );
}

pub fn collapse_ranges(nums: &[CnumIx]) -> Vec<RangePiece> {
    let mut pieces = Vec::new();
    if let Some(init) = nums.first() {
        let mut wip = RangePiece::Single(*init);
        for &num in &nums[1..] {
            if let Some(emit) = wip.attempt_append(num) {
                pieces.push(emit);
            }
        }
        pieces.push(wip);
    }
    pieces
}

#[test]
fn range_collapse() {
    let s = |cnum: u32| CnumIx::new(cnum, cnum as usize);
    assert_eq!(
        collapse_ranges(&[s(1), s(2), s(3)]),
        vec![RangePiece::Range(s(1), s(3))]
    );
    assert_eq!(
        collapse_ranges(&[s(1), s(2), CnumIx::new(4, 3)]),
        vec![
            RangePiece::Range(s(1), s(2)),
            RangePiece::Single(CnumIx::new(4, 3))
        ]
    );
}

pub struct Unnamed3<O: OutputFormat> {
    pub cite: Arc<Cite<O>>,
    pub cnum: Option<u32>,
    pub gen4: Arc<IrGen>,
    /// First of a group of cites with the same name
    pub is_first: bool,
    /// Subsequent in a group of cites with the same name
    pub should_collapse: bool,
    /// First of a group of cites with the same year, all with suffixes
    /// (same name implied)
    pub first_of_ys: bool,
    /// Subsequent in a group of cites with the same year, all with suffixes
    /// (same name implied)
    pub collapse_ys: bool,

    pub year_suffix: Option<u32>,

    /// Ranges of year suffixes (not alphabetic, in its base u32 form)
    pub collapsed_year_suffixes: Vec<RangePiece>,

    /// Ranges of citation numbers
    pub collapsed_ranges: Vec<RangePiece>,

    /// Tagging removed cites is cheaper than memmoving the rest of the Vec
    pub vanished: bool,

    pub has_locator: bool,
}

use std::fmt::{Debug, Formatter};

impl Debug for Unnamed3<Markup> {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        let fmt = &Markup::default();
        f.debug_struct("Unnamed3")
            .field("cite", &self.cite)
            .field("cnum", &self.cnum)
            .field(
                "gen4",
                &self.gen4.ir.flatten(&fmt).map(|x| fmt.output(x, false)),
            )
            .field("has_locator", &self.has_locator)
            .field("is_first", &self.is_first)
            .field("should_collapse", &self.should_collapse)
            .field("first_of_ys", &self.first_of_ys)
            .field("collapse_ys", &self.collapse_ys)
            .field("year_suffix", &self.year_suffix)
            .field("collapsed_year_suffixes", &self.collapsed_year_suffixes)
            .field("collapsed_ranges", &self.collapsed_ranges)
            .field("vanished", &self.vanished)
            .field("gen4_full", &self.gen4.ir)
            .finish()
    }
}

impl<O: OutputFormat> Unnamed3<O> {
    pub fn new(cite: Arc<Cite<O>>, cnum: Option<u32>, gen4: Arc<IrGen>) -> Self {
        Unnamed3 {
            has_locator: cite.locators.is_some() && gen4.ir.find_locator(),
            cite,
            gen4,
            cnum,
            is_first: false,
            should_collapse: false,
            first_of_ys: false,
            collapse_ys: false,
            year_suffix: None,
            collapsed_year_suffixes: Vec::new(),
            collapsed_ranges: Vec::new(),
            vanished: false,
        }
    }
}

pub fn group_and_collapse<O: OutputFormat<Output = String>>(
    db: &dyn IrDatabase,
    fmt: &Markup,
    delim: &str,
    collapse: Option<Collapse>,
    cites: &mut Vec<Unnamed3<O>>,
) {
    // Neat trick: same_names[None] tracks cites without a cs:names block, which helps with styles
    // that only include a year. (What kind of style is that?
    // magic_ImplicitYearSuffixExplicitDelimiter.txt, I guess that's the only possible reason, but
    // ok.)
    let mut same_names: HashMap<Option<String>, (usize, bool)> = HashMap::new();
    let mut same_years: HashMap<String, (usize, bool)> = HashMap::new();

    // First, group cites with the same name
    for ix in 0..cites.len() {
        let rendered = cites[ix]
            .gen4
            .ir
            .first_name_block()
            .and_then(|fnb| fnb.lock().unwrap().ir.flatten(fmt))
            .map(|flat| fmt.output(flat, false));
        same_names
            .entry(rendered)
            .and_modify(|(oix, seen_once)| {
                // Keep cites separated by affixes together
                if cites.get(*oix).map_or(false, |u| u.cite.has_suffix())
                    || cites.get(*oix + 1).map_or(false, |u| u.cite.has_prefix())
                    || cites.get(ix - 1).map_or(false, |u| u.cite.has_suffix())
                    || cites.get(ix).map_or(false, |u| u.cite.has_affix())
                {
                    *oix = ix;
                    *seen_once = false;
                    return;
                }
                if *oix < ix {
                    if !*seen_once {
                        cites[*oix].is_first = true;
                    }
                    *seen_once = true;
                    cites[ix].should_collapse = true;
                    let rotation = &mut cites[*oix + 1..ix + 1];
                    rotation.rotate_right(1);
                    *oix += 1;
                }
            })
            .or_insert((ix, false));
    }

    if collapse.map_or(false, |c| {
        c == Collapse::YearSuffixRanged || c == Collapse::YearSuffix
    }) {
        let mut top_ix = 0;
        while top_ix < cites.len() {
            if cites[top_ix].is_first {
                let mut moved = 0;
                let mut ix = top_ix;
                while ix < cites.len() {
                    if ix != top_ix && !cites[ix].should_collapse {
                        break;
                    }
                    moved += 1;
                    if let Some((y, suf)) = cites[ix]
                        .gen4
                        .ir
                        .find_first_year_and_suffix()
                        .map(|(y, suf)| (fmt.output(y, false), suf))
                    {
                        cites[ix].year_suffix = Some(suf);
                        same_years
                            .entry(y)
                            .and_modify(|(oix, seen_once)| {
                                if *oix == ix - 1 {
                                    if !*seen_once {
                                        cites[*oix].first_of_ys = true;
                                    }
                                    cites[ix].collapse_ys = true;
                                    *seen_once = true;
                                } else {
                                    *seen_once = false;
                                }
                                *oix = ix;
                            })
                            .or_insert((ix, false));
                    }
                    ix += 1;
                }
                top_ix += moved;
            }
            top_ix += 1;
        }
    }

    if collapse == Some(Collapse::CitationNumber) {
        // XXX: Gotta factor in that some might have prefixes and suffixes
        if let Some((first, rest)) = cites.split_first_mut() {
            first.is_first = true;
            for r in rest {
                r.should_collapse = true;
            }
        }
    }

    if let Some(collapse) = collapse {
        match collapse {
            Collapse::CitationNumber => {
                let mut ix = 0;
                while ix < cites.len() {
                    let slice = &mut cites[ix..];
                    if let Some((u, rest)) = slice.split_first_mut() {
                        if u.is_first {
                            let following = rest.iter_mut().take_while(|u| u.should_collapse);

                            let mut cnums = Vec::new();
                            if let Some(cnum) = u.cnum {
                                cnums.push(CnumIx::new(cnum, ix));
                            }
                            let mut count = 0;
                            for (nix, cite) in following.enumerate() {
                                if let Some(cnum) = cite.cnum {
                                    cnums.push(CnumIx {
                                        cnum,
                                        ix: ix + nix + 1,
                                        force_single: cite.has_locator,
                                    })
                                }
                                cite.vanished = true;
                                count += 1;
                            }
                            ix += count;
                            u.collapsed_ranges = collapse_ranges(&cnums);
                        }
                    }
                    ix += 1;
                }
            }
            Collapse::Year => {
                let mut ix = 0;
                while ix < cites.len() {
                    let slice = &mut cites[ix..];
                    if let Some((u, rest)) = slice.split_first_mut() {
                        if u.is_first {
                            let following = rest.iter_mut().take_while(|u| u.should_collapse);
                            let mut count = 0;
                            for (nix, cite) in following.enumerate() {
                                let gen4 = Arc::make_mut(&mut cite.gen4);
                                gen4.ir.suppress_names();
                                count += 1;
                            }
                            ix += count;
                        }
                    }
                    ix += 1;
                }
            }
            Collapse::YearSuffixRanged | Collapse::YearSuffix => {
                let mut ix = 0;
                while ix < cites.len() {
                    let slice = &mut cites[ix..];
                    if let Some((u, rest)) = slice.split_first_mut() {
                        if u.is_first {
                            let following = rest.iter_mut().take_while(|u| u.should_collapse);
                            for (nix, cite) in following.enumerate() {
                                let gen4 = Arc::make_mut(&mut cite.gen4);
                                gen4.ir.suppress_names();
                            }
                        }
                        if u.first_of_ys {
                            let following = rest.iter_mut().take_while(|u| u.collapse_ys);

                            if collapse == Collapse::YearSuffixRanged {
                                // Potentially confusing; 'cnums' here are year suffixes in u32 form.
                                let mut cnums = Vec::new();
                                if let Some(cnum) = u.year_suffix {
                                    cnums.push(CnumIx::new(cnum, ix));
                                }
                                for (nix, cite) in following.enumerate() {
                                    if let Some(cnum) = cite.year_suffix {
                                        cnums.push(CnumIx {
                                            cnum,
                                            ix: ix + nix + 1,
                                            force_single: cite.has_locator,
                                        });
                                    }
                                    cite.vanished = true;
                                    if !cite.has_locator {
                                        let gen4 = Arc::make_mut(&mut cite.gen4);
                                        gen4.ir.suppress_year();
                                    }
                                }
                                u.collapsed_year_suffixes = collapse_ranges(&cnums);
                            } else {
                                if let Some(cnum) = u.year_suffix {
                                    u.collapsed_year_suffixes
                                        .push(RangePiece::Single(CnumIx::new(cnum, ix)));
                                }
                                for (nix, cite) in following.enumerate() {
                                    if let Some(cnum) = cite.year_suffix {
                                        u.collapsed_year_suffixes.push(RangePiece::Single(
                                            CnumIx {
                                                cnum,
                                                ix: ix + nix + 1,
                                                force_single: cite.has_locator,
                                            },
                                        ));
                                    }
                                    cite.vanished = true;
                                    let gen4 = Arc::make_mut(&mut cite.gen4);
                                    gen4.ir.suppress_year();
                                }
                            }
                        }
                    }
                    ix += 1;
                }
            }
            _ => {}
        }
    }
}

fn pair_at_mut<T>(mut slice: &mut [T], ix: usize) -> Option<(&mut T, &mut T)> {
    let nix = ix + 1;
    slice = &mut slice[ix..];
    if slice.len() < 2 || nix >= slice.len() {
        return None;
    }
    slice
        .split_first_mut()
        .and_then(|(first, rest)| rest.first_mut().map(|second| (first, second)))
}

////////////////////////////////
// Cite Grouping & Collapsing //
////////////////////////////////

use csl::SubsequentAuthorSubstituteRule as SasRule;
use citeproc_io::PersonName;
use crate::disamb::names::{DisambNameRatchet, PersonDisambNameRatchet};

#[derive(Eq, PartialEq, Clone)]
pub enum ReducedNameToken<'a, B> {
    Name(&'a PersonName),
    Literal(&'a B),
    EtAl,
    Ellipsis,
    Delimiter,
    And,
    Space,
}

impl<'a, T: Debug>  Debug for ReducedNameToken<'a, T> {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            ReducedNameToken::Name(p) => write!(f, "{:?}", p.family),
            ReducedNameToken::Literal(b) => write!(f, "{:?}", b),
            ReducedNameToken::EtAl => write!(f, "EtAl"),
            ReducedNameToken::Ellipsis => write!(f, "Ellipsis"),
            ReducedNameToken::Delimiter => write!(f, "Delimiter"),
            ReducedNameToken::And => write!(f, "And"),
            ReducedNameToken::Space => write!(f, "Space"),
        }
    }
}

impl<'a, T> ReducedNameToken<'a, T> {
    fn from_token(token: &NameToken<'a, T>) -> Self {
        match token {
            NameToken::Name(dnr) => match dnr {
                DisambNameRatchet::Person(p) => ReducedNameToken::Name(&p.data.value),
                DisambNameRatchet::Literal(b) => ReducedNameToken::Literal(b),
            }
            NameToken::Ellipsis => ReducedNameToken::Ellipsis,
            NameToken::EtAl(..) => ReducedNameToken::EtAl,
            NameToken::Space => ReducedNameToken::Space,
            NameToken::Delimiter => ReducedNameToken::Delimiter,
            NameToken::And => ReducedNameToken::And,
        }
    }
    fn relevant(&self) -> bool {
        match self {
            ReducedNameToken::Name(_) | ReducedNameToken::Literal(_) => true,
            _ => false,
        }
    }
}

pub fn subsequent_author_substitute<O: OutputFormat>(
    fmt: &O,
    previous: &Mutex<NameIR<O>>,
    current: &Mutex<NameIR<O>>,
    sas: &str,
    sas_rule: SasRule,
) -> bool {
    let pre = previous.lock().unwrap();
    let mut cur = current.lock().unwrap();
    let pre_tokens = pre.iter_bib_rendered_names(fmt);
    let pre_reduced = pre_tokens
        .iter()
        .map(ReducedNameToken::from_token)
        .filter(|x| x.relevant());
    let cur_tokens = cur.iter_bib_rendered_names(fmt);
    let cur_reduced = cur_tokens
        .iter()
        .map(ReducedNameToken::from_token)
        .filter(|x| x.relevant());
    debug!("{:?} vs {:?}", pre_reduced.clone().collect::<Vec<_>>(), cur_reduced.clone().collect::<Vec<_>>());
    match sas_rule {
        SasRule::CompleteAll | SasRule::CompleteEach => {
            if Iterator::eq(pre_reduced, cur_reduced) {
                if sas_rule == SasRule::CompleteEach {
                    // let nir handle it
                    // u32::MAX so ALL names get --- treatment
                    cur.subsequent_author_substitute(fmt, std::u32::MAX, sas);
                } else if sas.is_empty() {
                    *cur.ir = IR::Rendered(None)
                } else {
                    let sas_ir = IR::Rendered(Some(CiteEdgeData::Output(fmt.plain(sas))));
                    let mut contents = vec![(sas_ir, GroupVars::Important)];
                    if let Some(label_el) = cur.names_inheritance.label.as_ref() {
                        if let Some(label) = cur.built_label.as_ref() {
                            let label_ir = IR::Rendered(Some(CiteEdgeData::Output(label.clone())));
                            if label_el.after_name {
                                contents.push((label_ir, GroupVars::Plain));
                            } else {
                                contents.insert(0, (label_ir, GroupVars::Plain));
                            }
                        }
                    }
                    *cur.ir = IR::Seq(IrSeq {
                        contents,
                        ..Default::default()
                    })
                };
                return true;
            }
        }
        SasRule::PartialEach => {
            let count = pre_reduced.zip(cur_reduced)
                .take_while(|(p, c)| p == c)
                .count();
            cur.subsequent_author_substitute(fmt, count as u32, sas);
        }
        SasRule::PartialFirst => {
            let count = pre_reduced.zip(cur_reduced)
                .take_while(|(p, c)| p == c)
                .count();
            if count > 0 {
                cur.subsequent_author_substitute(fmt, 1, sas);
            }
        }
    }
    false
}
