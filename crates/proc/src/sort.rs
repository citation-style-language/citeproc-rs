use unicase::UniCase;

use crate::db::{with_bib_context, with_cite_context};
use crate::prelude::*;
use citeproc_db::{ClusterData, ClusterId};
use citeproc_io::output::plain::PlainText;
use csl::*;
use fnv::FnvHashMap;
use std::sync::Arc;

fn plain_macro_element(macro_name: Atom) -> TextElement {
    TextElement {
        source: TextSource::Macro(macro_name),
        formatting: None,
        affixes: Default::default(),
        quotes: false,
        strip_periods: false,
        text_case: TextCase::None,
        display: None,
    }
}

pub fn sort_string_citation(
    db: &dyn IrDatabase,
    cite_id: CiteId,
    macro_name: Atom,
    key: SortKey,
) -> Option<Arc<SmartString>> {
    let bib_num = db.bib_number(cite_id);
    with_cite_context(db, cite_id, bib_num, Some(key), true, None, |ctx| {
        Arc::new(ctx_sort_string(ctx, macro_name, key))
    })
}

// Cached by the DB because typically the output needs to be compared more than once
pub fn sort_string_bibliography(
    db: &dyn IrDatabase,
    ref_id: Atom,
    macro_name: Atom,
    key: SortKey,
) -> Option<Arc<SmartString>> {
    with_bib_context(db, ref_id.clone(), None, Some(key), None, |_bib, ctx| {
        Arc::new(ctx_sort_string(ctx, macro_name, key))
    })
}

fn ctx_sort_string(ctx: &CiteContext<Markup, Markup>, macro_name: Atom) -> SmartString {
    let mut walker = SortingWalker::new(db, &ctx);
    let text = plain_macro_element(macro_name);
    let (string, _gv) = walker.text_macro(&text, &macro_name);
    string
}

pub fn sorted_refs(db: &dyn IrDatabase) -> Arc<(Vec<Atom>, FnvHashMap<Atom, u32>)> {
    let style = db.style();
    let bib = match style.bibliography {
        None => None,
        Some(ref b) => b.sort.as_ref(),
    };

    let mut citation_numbers = FnvHashMap::default();

    // Construct preordered, which will then be stably sorted. It contains:
    // - All refs from all cites, in the order they appear (excluding non-existent)
    // - Then, all of the uncited reference ids.
    //
    // first, compute refs in the order that they are cited.
    // stable sorting will cause this to be the final tiebreaker.
    let all = db.all_keys();
    let all_cite_ids = db.all_cite_ids();
    let uncited_ordered = db.uncited_ordered();
    let mut preordered = Vec::with_capacity(all.len());

    // Put all the cited refs in
    let mut i = 1;
    for &id in all_cite_ids.iter() {
        let ref_id = &id.lookup(db).ref_id;
        if all.contains(ref_id) && !citation_numbers.contains_key(ref_id) {
            preordered.push(ref_id.clone());
            citation_numbers.insert(ref_id.clone(), i as u32);
            i += 1;
        }
    }
    // Then all the uncited ones
    for id in uncited_ordered.iter() {
        // guaranteed to be a valid reference id already.
        // but may have duplicated an actual cite.
        preordered.push(id.clone());
        citation_numbers.insert(id.clone(), i as u32);
        i += 1;
    }
    let mut sort_cache = FnvHashMap::default();

    let refs = if let Some(ref sort) = bib {
        // dbg!(sort);
        // TODO: explore the sort_by_cached_key, but reimplement to have a closure sort function
        // rather than Ord.
        preordered.sort_by(|a, b| {
            let a_cnum = citation_numbers.get(a).unwrap();
            let b_cnum = citation_numbers.get(b).unwrap();
            with_bib_context(db, a.clone(), Some(*a_cnum), None, None, |_, a_ctx| {
                with_bib_context(db, b.clone(), Some(*b_cnum), None, None, |_, b_ctx| {
                    bib_ordering(
                        db,
                        &mut sort_cache,
                        CiteOrBib::Bibliography,
                        (a, &mut a_ctx, *a_cnum),
                        (b, &mut b_ctx, *b_cnum),
                        sort,
                    )
                })
            })
            .flatten()
            .unwrap_or(Ordering::Equal)
        });
        preordered
    } else {
        // In the absence of cs:sort, cites and bibliographic entries appear in the order in which
        // they are cited. The uncited ones come last.
        preordered
    };
    for (i, ref_id) in refs.iter().enumerate() {
        citation_numbers.insert(ref_id.clone(), (i + 1) as u32);
    }
    Arc::new((refs, citation_numbers))
}

pub fn clusters_cites_sorted(db: &dyn IrDatabase) -> Arc<Vec<ClusterData>> {
    let cluster_ids = db.cluster_ids();
    let mut clusters: Vec<_> = cluster_ids
        .iter()
        // No number? Not considered to be in document, position participant.
        // Although may be disamb participant.
        .filter_map(|&id| db.cluster_data_sorted(id))
        .collect();
    clusters.sort_by_key(|cluster| cluster.number);
    Arc::new(clusters)
}

pub fn cluster_data_sorted(db: &dyn IrDatabase, id: ClusterId) -> Option<ClusterData> {
    db.cluster_note_number(id).map(|number| {
        // Order of operations: bib gets sorted first, so cites can be sorted by
        // citation-number.
        let sorted_refs_arc = db.sorted_refs();
        let (_keys, citation_numbers_by_id) = &*sorted_refs_arc;
        let mut cites = db.cluster_cites(id);
        let style = db.style();
        if let Some(sort) = style.citation.sort.as_ref() {
            let mut neu = (*cites).clone();
            let getter = |cite_id: &CiteId| -> Option<u32> {
                let cite = cite_id.lookup(db);
                let cnum = db.reference(cite.ref_id.clone()).map(|refr| {
                    *citation_numbers_by_id
                        .get(&refr.id)
                        .expect("sorted_refs should contain a bib_item key")
                });
                cnum
            };
            let mut sort_cache = FnvHashMap::default();
            neu.sort_by(|a_id, b_id| match (getter(a_id), getter(b_id)) {
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
                (Some(a_cnum), Some(b_cnum)) => {
                    with_cite_context(db, *a_id, Some(a_cnum), None, true, None, |mut a_ctx| {
                        with_cite_context(db, *b_id, Some(b_cnum), None, true, None, |mut b_ctx| {
                            bib_ordering(
                                db,
                                sort_cache,
                                CiteOrBib::Bibliography,
                                (*a_id, &mut a_ctx, a_cnum),
                                (*b_id, &mut b_ctx, b_cnum),
                                sort,
                            )
                        })
                    })
                    .flatten()
                    .unwrap_or(Ordering::Equal)
                }
            });
            cites = Arc::new(neu);
        }
        ClusterData { id, number, cites }
    })
}

pub fn bib_number(db: &dyn IrDatabase, id: CiteId) -> Option<u32> {
    let cite = id.lookup(db);
    let arc = db.sorted_refs();
    let (_, ref lookup_ref_ids) = &*arc;
    lookup_ref_ids.get(&cite.ref_id).cloned()
}

#[derive(PartialEq, Eq)]
enum SortItem {
    Macro(NaturalCmp),
    OrdinaryVariable(UniCase<SmartString>),
    Cnum(u32),
    Number(citeproc_io::NumericValueOwned),
    Names(Option<Vec<UniCase<SmartString>>>),
    Date(DateOrRange),
}

use std::cmp::Ordering;
#[derive(Debug)]
enum Demoted {
    Left,
    Right,
}
use natural_sort::NaturalCmp;

#[derive(Eq)]
struct Demoting<T> {
    // ignored in partialeq/eq/ord equivalence
    direction: SortDirection,
    option: Option<T>
}

impl<T: Eq> PartialEq for Demoting<T> {
    fn eq(&self, other: &Self) -> bool {
        self.option == other.option
    }
}

impl<T: Eq + Ord> Ord for Demoting<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        let (ordering, demoted) = compare_demoting_none(&self.option, &other.option);
        // Necessary for Ord implementation contract
        // These will always be the same as we always build the two vecs from the same sort keys
        debug_assert_eq!(self.direction, other.direction);
        match (self.direction, demoted) {
            // Wants to be reversed, but overridden by demotion
            (_, Some(Demoted::Left)) => Ordering::Greater,
            (_, Some(Demoted::Right)) => Ordering::Less,
            (SortDirection::Descending, _) => ordering.reverse(),
            _ => ordering,
        }
    }
}

fn compare_demoting_none<T: PartialOrd>(
    aa: Option<T>,
    bb: Option<T>,
) -> (Ordering, Option<Demoted>) {
    match (aa, bb) {
        (None, None) => (Ordering::Equal, None),
        (None, Some(_)) => (Ordering::Greater, Some(Demoted::Left)),
        (Some(_), None) => (Ordering::Less, Some(Demoted::Right)),
        (Some(aaa), Some(bbb)) => (aaa.partial_cmp(&bbb).unwrap_or(Ordering::Equal), None),
    }
}

pub fn ctx_sort_items<ID, O, I>(
    db: &dyn IrDatabase,
    sort_cache: &mut FnvHashMap<(ID, Atom, SortKey), Option<Arc<SmartString>>>,
    // Cached lookup from (id, macro name, sort key) -> a comparable string
    cite_or_bib: CiteOrBib,
    a_id: ID,
    a_ctx: &mut CiteContext<'_, O, I>
    a_cnum: u32,
    sort: &Sort,
)
where
    ID: Copy + Eq + std::hash::Hash + Debug,
    O: OutputFormat,
    I: OutputFormat,
{
    let sort_string = |ctx: &mut CiteContext<Markup, Markup>, macro_name: Atom, key: SortKey, cnum: u32| {
        ctx.bib_number = Some(cnum);
        if cite_or_bib == CiteOrBib::Bibliography {
            ctx.sort_key = Some(key);
            ctx_sort_string(ctx, macro_name)
        } else {
            ctx.sort_key = Some(key);
            ctx_sort_string(ctx, macro_name)
        }
    };
    let items = Vec::with_capacity(sort.keys.len());
    for key in sort.keys.iter() {
        let (o, demoted) = match key.sort_source {
            SortSource::Macro(ref macro_name) => {
                let a_string = sort_string(a_ctx, macro_name.clone(), key.clone(), a_cnum);
                let a_nat = NaturalCmp::new(a_string);
                SortItem::Macro(a_nat)
            }
            // For variables, we're not going to use the CiteContext wrappers, because if a
            // variable is not defined directly on the reference, it shouldn't be sortable-by, so
            // will just come back as None from reference.xxx.get() and produce Equal.
            SortSource::Variable(any) => match any {
                AnyVariable::Ordinary(v) => {
                    use citeproc_io::micro_html_to_string;
                    fn strip_markup(s: impl AsRef<str>) -> SmartString {
                        micro_html_to_string(s.as_ref(), &Default::default())
                    };
                    a_ctx
                        .get_ordinary(v, VariableForm::default())
                        .map(strip_markup)
                        .map(UniCase::new);
                }
                AnyVariable::Number(NumberVariable::CitationNumber) => {
                    SortItem::Cnum(a_cnum)
                }
                AnyVariable::Number(v) => {
                    SortItem::Number(a_ctx.get_number(v).into())
                    compare_demoting_none(a_ctx.get_number(v), b_ctx.get_number(v))
                }
                AnyVariable::Name(v) => {
                    let a_strings =
                        crate::names::sort_strings_for_names(db, &a_ctx.reference, v, key, cite_or_bib);
                    let b_strings =
                        crate::names::sort_strings_for_names(db, &b_ctx.reference, v, key, cite_or_bib);
                    let x = compare_demoting_none(a_strings.as_ref(), b_strings.as_ref());
                    debug!(
                        "cmp names {:?}: {:?} {:?} {:?} {:?} {:?}",
                        v, a_id, a_strings, x.0, b_id, b_strings
                    );
                    x
                }
                // TODO: compare dates, using details from spec for ranges
                AnyVariable::Date(v) => {
                    let a_date = a_ctx.reference.date.get(&v);
                    let b_date = b_ctx.reference.date.get(&v);
                    compare_demoting_none(a_date, b_date)
                }
            },
        };
        ord = match (key.direction.as_ref(), demoted) {
            // Wants to be reversed, but overridden by demotion
            (_, Some(Demoted::Left)) => Ordering::Greater,
            (_, Some(Demoted::Right)) => Ordering::Less,
            (Some(SortDirection::Descending), _) => o.reverse(),
            _ => o,
        };
    }
}

/// Creates a total ordering of References from a Sort element. (Not a query)
pub fn bib_ordering<
    ID: std::fmt::Debug + Copy + Eq + std::hash::Hash,
    O: OutputFormat,
    I: OutputFormat,
>(
    db: &dyn IrDatabase,
    sort_cache: &mut FnvHashMap<(ID, Atom, SortKey), Option<Arc<SmartString>>>,
    // Cached lookup from (id, macro name, sort key) -> a comparable string
    cite_or_bib: CiteOrBib,
    a_id_ctx_cnum: (ID, &mut CiteContext<'_, O, I>, u32),
    b_id_ctx_cnum: (ID, &mut CiteContext<'_, O, I>, u32),
    sort: &Sort,
) -> Vec<SortIten> {
    let mut ord = Ordering::Equal;
    let (a_id, a_ctx, a_cnum) = a_id_ctx_cnum;
    let (b_id, b_ctx, b_cnum) = b_id_ctx_cnum;
    let mut cached_sort_string = |ctx: &mut CiteContext<Markup, Markup>, macro_name: Atom, key: SortKey, cnum: u32| {
        sort_cache
            .entry((a_id, macro_name.clone(), key.clone()))
            .or_insert_with(|| {
                ctx.bib_number = Some(cnum);
                if cite_or_bib == CiteOrBib::Bibliography {
                    ctx.sort_key = Some(key);
                    ctx_sort_string(ctx, macro_name)
                } else {
                    ctx.sort_key = Some(key);
                    ctx_sort_string(ctx, macro_name)
                }
            })
    };
    for key in sort.keys.iter() {
        let (o, demoted) = match key.sort_source {
            SortSource::Macro(ref macro_name) => {
                let a_string = cached_sort_string(a_ctx, macro_name.clone(), key.clone(), a_cnum);
                let b_string = cached_sort_string(b_ctx, macro_name.clone(), key.clone(), b_cnum);
                let a_nat = NaturalCmp::new(a_string);
                let b_nat = NaturalCmp::new(b_string);
                let x = compare_demoting_none(a_nat, b_nat);
                debug!(
                    "cmp macro {}: {:?} {:?} {:?} {:?} {:?}",
                    macro_name, a_id, a_string, x.0, b_id, b_string
                );
                x
            }
            // For variables, we're not going to use the CiteContext wrappers, because if a
            // variable is not defined directly on the reference, it shouldn't be sortable-by, so
            // will just come back as None from reference.xxx.get() and produce Equal.
            SortSource::Variable(any) => match any {
                AnyVariable::Ordinary(v) => {
                    use citeproc_io::micro_html_to_string;
                    fn strip_markup(s: impl AsRef<str>) -> SmartString {
                        micro_html_to_string(s.as_ref(), &Default::default())
                    };
                    let aa = a_ctx
                        .get_ordinary(v, VariableForm::default())
                        .map(strip_markup)
                        .map(UniCase::new);
                    let bb = b_ctx
                        .get_ordinary(v, VariableForm::default())
                        .map(strip_markup)
                        .map(UniCase::new);
                    let x = compare_demoting_none(aa.as_ref(), bb.as_ref());
                    debug!(
                        "cmp ordinary {:?}: {:?} {:?} {:?} {:?} {:?}",
                        v,
                        a_ctx.reference.id,
                        aa.as_ref(),
                        x.0,
                        b_ctx.reference.id,
                        bb.as_ref()
                    );
                    x
                }
                AnyVariable::Number(NumberVariable::CitationNumber) => {
                    compare_demoting_none(Some(a_cnum), Some(b_cnum))
                }
                AnyVariable::Number(v) => {
                    compare_demoting_none(a_ctx.get_number(v), b_ctx.get_number(v))
                }
                AnyVariable::Name(v) => {
                    let a_strings =
                        crate::names::sort_strings_for_names(db, &a_ctx.reference, v, key, cite_or_bib);
                    let b_strings =
                        crate::names::sort_strings_for_names(db, &b_ctx.reference, v, key, cite_or_bib);
                    let x = compare_demoting_none(a_strings.as_ref(), b_strings.as_ref());
                    debug!(
                        "cmp names {:?}: {:?} {:?} {:?} {:?} {:?}",
                        v, a_id, a_strings, x.0, b_id, b_strings
                    );
                    x
                }
                // TODO: compare dates, using details from spec for ranges
                AnyVariable::Date(v) => {
                    let a_date = a_ctx.reference.date.get(&v);
                    let b_date = b_ctx.reference.date.get(&v);
                    compare_demoting_none(a_date, b_date)
                }
            },
        };
        ord = match (key.direction.as_ref(), demoted) {
            // Wants to be reversed, but overridden by demotion
            (_, Some(Demoted::Left)) => Ordering::Greater,
            (_, Some(Demoted::Right)) => Ordering::Less,
            (Some(SortDirection::Descending), _) => o.reverse(),
            _ => o,
        };
    }
    ord
}

/// Currently only works where
struct SortingWalker<'a, I: OutputFormat> {
    db: &'a dyn IrDatabase,
    /// the cite is in its original format, but the formatter is PlainText
    ctx: CiteContext<'a, PlainText, I>,
    state: IrState,
    /// Use this for generating names and dates, and not creating a new one each time
    arena: IrArena<PlainText>,
}

impl<'a, I: OutputFormat> SortingWalker<'a, I> {
    pub fn new<O: OutputFormat>(db: &'a dyn IrDatabase, ctx: &'a CiteContext<'a, O, I>) -> Self {
        let plain_ctx = ctx.change_format(PlainText);
        SortingWalker {
            db,
            ctx: plain_ctx,
            state: Default::default(),
            arena: Default::default(),
        }
    }

    fn renderer(&'a self) -> Renderer<'a, PlainText, I> {
        Renderer::gen(GenericContext::Cit(&self.ctx))
    }
}

#[test]
fn test_date_as_macro_strip_delims() {
    use crate::test::MockProcessor;
    let mut db = MockProcessor::new();
    let mut refr = citeproc_io::Reference::empty("ref_id".into(), CslType::Book);
    use citeproc_io::{Date, DateOrRange};
    refr.ordinary.insert(Variable::Title, String::from("title"));
    refr.date.insert(
        DateVariable::Issued,
        DateOrRange::Single(Date::new(2000, 1, 1)),
    );
    db.insert_references(vec![refr]);
    db.set_style_text(r#"<?xml version="1.0" encoding="utf-8"?>
        <style version="1.0" class="note">
           <macro name="year-date">
               <date variable="issued">
                 <date-part name="year" />
               </date>
           </macro>
           <macro name="year-date-choose">
             <choose>
                 <if variable="issued">
                    <date variable="issued">
                       <date-part name="year"/>
                    </date>
                 </if>
                 <else>
                    <text term="no date" form="short"/>
                 </else>
              </choose>
           </macro>
           <macro name="local">
               <date variable="issued" date-parts="year" form="numeric"/>
           </macro>
           <macro name="term">
             <text term="anonymous"/>
           </macro>
           <macro name="indep">
             <text variable="title" />
             <date variable="issued">
               <date-part name="year" form="short" prefix="PREFIX" suffix="SUFFIX" />
               <date-part name="month" form="long" vertical-align="sup" prefix="PREFIX" suffix="SUFFIX" />
             </date>
           </macro>
           <citation><layout></layout></citation>
           <bibliography>
             <sort>
               <key macro="indep" />
             </sort>
             <layout>
             </layout>
           </bibliography>
        </style>
    "#);

    assert_eq!(
        sort_string_bibliography(
            &db,
            "ref_id".into(),
            "indep".into(),
            SortKey::macro_named("indep")
        ),
        Some(Arc::new("title\u{e000}2000_01/0000_00\u{e001}".into()))
    );

    assert_eq!(
        sort_string_bibliography(
            &db,
            "ref_id".into(),
            "local".into(),
            SortKey::macro_named("local")
        ),
        Some(Arc::new("\u{e000}2000_/0000_\u{e001}".into()))
    );

    assert_eq!(
        sort_string_bibliography(
            &db,
            "ref_id".into(),
            "year-date".into(),
            SortKey::macro_named("year-date")
        ),
        Some(Arc::new("\u{e000}2000_/0000_\u{e001}".into()))
    );

    assert_eq!(
        sort_string_bibliography(
            &db,
            "ref_id".into(),
            "year-date-choose".into(),
            SortKey::macro_named("year-date-choose")
        ),
        Some(Arc::new("\u{e000}2000_/0000_\u{e001}".into()))
    );

    assert_eq!(
        sort_string_bibliography(
            &db,
            "ref_id".into(),
            "term".into(),
            SortKey::macro_named("term")
        ),
        Some(Arc::new("anonymous".into()))
    );
}

impl<'a, O: OutputFormat> StyleWalker for SortingWalker<'a, O> {
    type Output = (SmartString, GroupVars);
    type Checker = CiteContext<'a, PlainText, O>;

    fn default(&mut self) -> Self::Output {
        Default::default()
    }
    fn get_checker(&self) -> Option<&Self::Checker> {
        Some(&self.ctx)
    }

    fn fold(&mut self, elements: &[Element], fold_type: WalkerFoldType) -> Self::Output {
        let iter = elements.iter();
        let mut output: Option<SmartString> = None;
        // Avoid allocating one new string
        let mut gv_acc = GroupVars::new();
        for el in iter {
            let (child, gv) = self.element(el);
            gv_acc = gv_acc.neighbour(gv);
            output = match output {
                Some(ref mut s) => {
                    s.push_str(&child);
                    continue;
                }
                None => Some(child),
            }
        }
        let out = output.unwrap_or_default();
        match fold_type {
            WalkerFoldType::Group(_g) => gv_acc.implicit_conditional(out),
            _ => (out, gv_acc),
        }
    }

    fn text_value(&mut self, text: &TextElement, value: &Atom) -> Self::Output {
        let renderer = self.renderer();
        let val = renderer.text_value(text, &value);
        (val.unwrap_or_default(), GroupVars::new())
    }

    fn text_term(
        &mut self,
        text: &TextElement,
        sel: TextTermSelector,
        plural: bool,
    ) -> Self::Output {
        let renderer = self.renderer();
        let val = renderer.text_term(text, sel, plural);
        (val.unwrap_or_default(), GroupVars::new())
    }

    // TODO: reinstate variable suppression
    fn text_variable(
        &mut self,
        text: &TextElement,
        svar: StandardVariable,
        form: VariableForm,
    ) -> Self::Output {
        let renderer = self.renderer();
        let res = match svar {
            StandardVariable::Number(nvar) => self
                .ctx
                .get_number(nvar)
                .map(|nval| renderer.text_variable(text, svar, nval.verbatim())),
            StandardVariable::Ordinary(var) => self
                .ctx
                .get_ordinary(var, form)
                .map(|val| renderer.text_variable(text, svar, &val)),
        };
        let gv = GroupVars::rendered_if(res.is_some());
        (res.unwrap_or_default(), gv)
    }

    // TODO: reinstate variable suppression
    fn number(&mut self, number: &NumberElement) -> Self::Output {
        let renderer = self.renderer();
        let var = number.variable;
        let content = self.ctx.get_number(var).map(|val| {
            renderer.number_sort_string(
                var,
                number.form,
                &val,
                number.affixes.as_ref(),
                number.text_case,
            )
        });
        let gv = GroupVars::rendered_if(content.is_some());
        (content.unwrap_or_default(), gv)
    }

    fn label(&mut self, label: &LabelElement) -> Self::Output {
        let renderer = self.renderer();
        let var = label.variable;
        let content = self
            .ctx
            .get_number(var)
            .and_then(|val| renderer.numeric_label(label, &val));
        (content.unwrap_or_default(), GroupVars::new())
    }

    // SPEC:
    // For name sorting, there are four advantages in using the same macro rendering
    // and sorting, instead of sorting directly on the name variable.
    //
    // 1.  First, substitution is available (e.g. the "editor" variable might
    //     substitute for an empty "author" variable).
    // 2.  Secondly, et-al abbreviation can be used (using either the
    //     et-al-min/et-al-subsequent-min, et-al-use-first/et-al-subsequent-use-first,
    //     and et-al-use-last options defined for the macro, or the overriding
    //     names-min, names-use-first and names-use-last attributes set on cs:key).
    //     When et-al abbreviation occurs, the "et-al" and "and others" terms are
    //     excluded from the sort key values.
    // 3.  Thirdly, names can be sorted by just the surname (using a macro for which
    //     the form attribute on cs:name is set to "short").
    // 4.  Finally, it is possible to sort by the number of names in a name list, by
    //     calling a macro for which the form attribute on cs:name is set to "count".
    //     As for names sorted via the variable attribute, names sorted via macro are
    //     returned with the cs:name attribute name-as-sort-order set to "all".
    //
    //     So
    //
    //     1. Override naso = all,
    //     2. Exclude et-al and & others terms,
    //     3. Return count as a {:08} padded number

    fn names(&mut self, names: &Names) -> Self::Output {
        let node =
            crate::names::intermediate(names, self.db, &mut self.state, &self.ctx, &mut self.arena);
        let gv = self.arena.get(node).unwrap().get().1;
        (
            IR::flatten(node, &self.arena, &self.ctx.format).unwrap_or_default(),
            gv,
        )
    }

    // The spec is not functional. Specificlly, negative/BCE years won't work. So the year must be
    // interpreted as a number, and the rest can still be a string. Hence CmpDate below.
    //
    fn date(&mut self, date: &BodyDate) -> Self::Output {
        let node = date.intermediate(self.db, &mut self.state, &self.ctx, &mut self.arena);
        let gv = self.arena.get(node).unwrap().get().1;
        (
            IR::flatten(node, &self.arena, &self.ctx.format).unwrap_or_default(),
            gv,
        )
    }

    fn text_macro(&mut self, text: &TextElement, name: &Atom) -> Self::Output {
        // TODO: same todos as in Proc
        let style = self.ctx.style;
        let macro_elements = style
            .macros
            .get(name)
            .expect("macro errors not implemented!");

        self.state.push_macro(name);
        let ret = self.fold(macro_elements, WalkerFoldType::Macro(text));
        self.state.pop_macro(name);
        ret
    }
}

// dates: Date variables called via the variable attribute are returned in the YYYYMMDD format,
// with zeros substituted for any missing date-parts (e.g. 20001200 for December 2000). As a
// result, less specific dates precede more specific dates in ascending sorts, e.g. “2000, May
// 2000, May 1st 2000”. Negative years are sorted inversely, e.g. “100BC, 50BC, 50AD, 100AD”.
// Seasons are ignored for sorting, as the chronological order of the seasons differs between the
// northern and southern hemispheres. In the case of date ranges, the start date is used for the
// primary sort, and the end date is used for a secondary sort, e.g. “2000–2001, 2000–2005,
// 2002–2003, 2002–2009”. Date ranges are placed after single dates when they share the same
// (start) date, e.g. “2000, 2000–2002”.

// Basically, everything would be very easy without the BC/AD sorting and the ranges coming later
// parts. But given these, we have to parse dates again.
pub mod natural_sort {

    // From the BMP(0) unicode private use area
    // Delimits a date so it can be parsed when doing a natural sort comparison
    pub const DATE_START: char = '\u{E000}';
    pub const DATE_START_STR: &str = "\u{E000}";
    pub const DATE_END: char = '\u{E001}';
    pub const DATE_END_STR: &str = "\u{E001}";

    // Delimits a number so it can be compared
    pub const NUM_START: char = '\u{E002}';
    pub const NUM_START_STR: &str = "\u{E002}";
    pub const NUM_END: char = '\u{E003}';
    pub const NUM_END_STR: &str = "\u{E003}";

    pub fn date_affixes() -> Affixes {
        Affixes {
            prefix: DATE_START_STR.into(),
            suffix: DATE_END_STR.into(),
        }
    }

    pub fn num_affixes() -> Affixes {
        Affixes {
            prefix: NUM_START_STR.into(),
            suffix: NUM_END_STR.into(),
        }
    }

    #[derive(PartialEq, Eq, Debug)]
    struct CmpDate<'a> {
        year: Option<i32>,
        rest: &'a str,
    }

    impl<'a> Ord for CmpDate<'a> {
        fn cmp(&self, other: &Self) -> Ordering {
            self.year
                .cmp(&other.year)
                .then_with(|| self.rest.cmp(other.rest))
        }
    }

    impl<'a> PartialOrd for CmpDate<'a> {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(
                self.year
                    .cmp(&other.year)
                    .then_with(|| self.rest.cmp(other.rest)),
            )
        }
    }

    #[derive(PartialEq, Eq, Debug)]
    enum CmpRange<'a> {
        Single(CmpDate<'a>),
        Range(CmpDate<'a>, CmpDate<'a>),
    }

    impl<'a> Ord for CmpRange<'a> {
        fn cmp(&self, other: &Self) -> Ordering {
            match (self, other) {
                (CmpRange::Single(a), CmpRange::Single(b)) => a.cmp(b),
                (CmpRange::Single(a), CmpRange::Range(b, _c)) => a.cmp(b),
                (CmpRange::Range(a, _b), CmpRange::Single(c)) => a.cmp(c),
                (CmpRange::Range(a, b), CmpRange::Range(c, d)) => a.cmp(c).then_with(|| b.cmp(d)),
            }
        }
    }

    impl<'a> PartialOrd for CmpRange<'a> {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    use csl::Affixes;
    use nom::{
        branch::alt,
        bytes::complete::{take_while, take_while1, take_while_m_n},
        character::complete::char,
        combinator::{map, opt},
        sequence::delimited,
        IResult,
    };
    use std::cmp::Ordering;
    use std::str::FromStr;

    fn to_u32(s: &str) -> u32 {
        FromStr::from_str(s).unwrap()
    }

    fn to_i32(s: &str) -> i32 {
        FromStr::from_str(s).unwrap()
    }

    fn take_8_digits(inp: &str) -> IResult<&str, &str> {
        take_while_m_n(1, 8, |c: char| c.is_ascii_digit())(inp)
    }

    fn year_prefix(inp: &str) -> IResult<&str, char> {
        alt((char('+'), char('-')))(inp)
    }

    fn year(inp: &str) -> IResult<&str, i32> {
        let (rem1, pref) = opt(year_prefix)(inp)?;
        let (rem2, y) = take_while1(|c: char| c.is_ascii_digit())(rem1)?;
        let (rem3, _) = char('_')(rem2)?;
        Ok((
            rem3,
            match pref {
                Some('-') => -to_i32(y),
                _ => to_i32(y),
            },
        ))
    }

    fn date(inp: &str) -> IResult<&str, CmpDate> {
        let (rem1, year) = opt(year)(inp)?;
        fn still_date(c: char) -> bool {
            c != DATE_END && c != '/'
        }
        let (rem2, rest) = take_while(still_date)(rem1)?;
        Ok((rem2, CmpDate { year, rest }))
    }

    fn range(inp: &str) -> IResult<&str, Token> {
        let (rem1, _) = char(DATE_START)(inp)?;
        let (rem2, first) = date(rem1)?;
        fn and_ymd(inp: &str) -> IResult<&str, CmpDate> {
            let (rem1, _) = char('/')(inp)?;
            Ok(date(rem1)?)
        }
        let (rem3, d2) = opt(and_ymd)(rem2)?;
        let (rem4, _) = char(DATE_END)(rem3)?;
        Ok((
            rem4,
            Token::Date(match d2 {
                None => CmpRange::Single(first),
                Some(d) => CmpRange::Range(first, d),
            }),
        ))
    }

    fn num(inp: &str) -> IResult<&str, Token> {
        delimited(
            char(NUM_START),
            map(take_8_digits, |x| Token::Num(to_u32(x))),
            char(NUM_END),
        )(inp)
    }

    fn str_token(inp: &str) -> IResult<&str, Token> {
        fn normal(c: char) -> bool {
            !(c == DATE_START || c == NUM_START)
        }
        map(take_while1(normal), Token::Str)(inp)
    }

    fn token(inp: &str) -> IResult<&str, Token> {
        alt((str_token, num, range))(inp)
    }

    struct TokenIterator<'a> {
        remain: &'a str,
    }

    #[derive(PartialEq, Debug)]
    enum Token<'a> {
        Str(&'a str),
        Num(u32),
        Date(CmpRange<'a>),
    }

    impl<'a> PartialOrd for Token<'a> {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            use unicase::UniCase;
            match (self, other) {
                (Token::Str(a), Token::Str(b)) => UniCase::new(a).partial_cmp(&UniCase::new(b)),
                (Token::Date(a), Token::Date(b)) => a.partial_cmp(b),
                (Token::Num(a), Token::Num(b)) => a.partial_cmp(b),
                _ => None,
            }
        }
    }

    impl<'a> Iterator for TokenIterator<'a> {
        type Item = Token<'a>;
        fn next(&mut self) -> Option<Self::Item> {
            if self.remain.is_empty() {
                return None;
            }
            if let Ok((remainder, token)) = token(self.remain) {
                self.remain = remainder;
                Some(token)
            } else {
                None
            }
        }
    }

    #[derive(PartialEq, Eq)]
    pub struct NaturalCmp<'a>(&'a str);
    impl<'a> NaturalCmp<'a> {
        pub fn new(s: &'a str) -> Option<Self> {
            if s.is_empty() {
                None
            } else {
                Some(NaturalCmp(s))
            }
        }
    }
    impl<'a> PartialOrd for NaturalCmp<'a> {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }
    impl<'a> Ord for NaturalCmp<'a> {
        fn cmp(&self, other: &Self) -> Ordering {
            natural_cmp(self.0, other.0)
        }
    }

    fn natural_cmp(a: &str, b: &str) -> Ordering {
        let a_i = TokenIterator { remain: a };
        let b_i = TokenIterator { remain: b };
        let mut iter = a_i.zip(b_i);
        let mut o = Ordering::Equal;
        while let Some((a_t, b_t)) = iter.next() {
            if o != Ordering::Equal {
                return o;
            }
            if let Some(c) = a_t.partial_cmp(&b_t) {
                o = c;
            }
        }
        o
    }

    #[test]
    fn natural_cmp_strings() {
        assert_eq!(natural_cmp("a", "z"), Ordering::Less, "a - z");
        assert_eq!(natural_cmp("z", "a"), Ordering::Greater, "z - a");
        assert_eq!(
            natural_cmp("a\u{E000}2009_0407\u{E001}", "a\u{E000}2008_0407\u{E001}"),
            Ordering::Greater,
            "2009 > 2008"
        );
        assert_eq!(
            natural_cmp("a\u{E000}2009_0507\u{E001}", "a\u{E000}2009_0407\u{E001}"),
            Ordering::Greater
        );
        assert_eq!(
            natural_cmp("a\u{E000}-0100_\u{E001}", "a\u{E000}0100_\u{E001}"),
            Ordering::Less,
            "100BC < 100AD"
        );

        // 2000, May 2000, May 1st 2000
        assert_eq!(
            natural_cmp("a\u{E000}2000_\u{E001}", "a\u{E000}2000_04\u{E001}"),
            Ordering::Less,
            "2000 < May 2000"
        );
        assert_eq!(
            natural_cmp("a\u{E000}2000_04\u{E001}", "a\u{E000}2000_0401\u{E001}"),
            Ordering::Less,
            "May 2000 < May 1st 2000"
        );

        assert_eq!(
            natural_cmp(
                "a\u{E000}2009_0407/0000_0000\u{E001}",
                "a\u{E000}2009_0407/2010_0509\u{E001}"
            ),
            Ordering::Less,
            "2009 < 2009/2010"
        );

        assert_eq!(
            natural_cmp(
                "\u{e000}-044_0315/0000_00\u{e001}",
                "\u{e000}-100_0713/0000_00\u{e001}"
            ),
            Ordering::Greater,
            "44BC > 100BC"
        );

        // Numbers
        assert_eq!(
            natural_cmp("\u{E002}1000\u{E003}", "\u{E002}1000\u{E003}"),
            Ordering::Equal,
            "1000 == 1000"
        );
        assert_eq!(
            natural_cmp("\u{E002}1000\u{E003}", "\u{E002}2000\u{E003}"),
            Ordering::Less,
            "1000 < 2000"
        );

        // Case insensitive
        assert_eq!(natural_cmp("aaa", "AAA"), Ordering::Equal);
    }
}
