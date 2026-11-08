//! Structured catalogue sizes.
//!
//! LS-Marker stores `thread`, `length_mm`, `af_mm`, `width_mm`, `height_mm`,
//! `od_mm`, `id_mm`, and `thickness_mm` on each part. The DINO catalogue also
//! carries `length_rule`, `diameter_rule`, and a note. The rules name which of
//! those fields is the length and which is the diameter. A free-text Dimensions
//! string is display text only and is not read for a number. Older catalogues
//! still carry a list of `{ "name", "mm" }` or an object of named sizes. Those
//! names are mapped onto the same fields. A description is never parsed.

use serde_json::{Map, Value};

#[derive(Clone, Debug, Default, PartialEq)]
struct Metrics {
    thread: String,
    thread_mm: Option<f64>,
    length_mm: Option<f64>,
    af_mm: Option<f64>,
    width_mm: Option<f64>,
    height_mm: Option<f64>,
    od_mm: Option<f64>,
    id_mm: Option<f64>,
    thickness_mm: Option<f64>,
    head_type: String,
}

enum Slot {
    Thread,
    ThreadMm,
    Length,
    Af,
    Width,
    Height,
    Od,
    Id,
    Thickness,
    Head,
    Skip,
}

/// Sizes written beside a part, or inside `dimensions` when that value is an
/// object or a `{ name, mm }` list. A string is ignored.
pub(crate) fn from_part(method: &str, kind: &str, map: &Map<String, Value>) -> Value {
    let family = family(method, kind);
    let mut metrics = Metrics::default();
    absorb_top(&mut metrics, family, map);
    absorb_dim_slot(&mut metrics, family, map, "dim1_name", "dim1_mm");
    absorb_dim_slot(&mut metrics, family, map, "dim2_name", "dim2_mm");
    absorb_dim_slot(&mut metrics, family, map, "dim3_name", "dim3_mm");
    if let Some(value) = map
        .get("dimensions")
        .or_else(|| map.get("dimension"))
        .or_else(|| map.get("size"))
    {
        absorb_value(&mut metrics, family, value);
    }
    finish(&mut metrics);
    pack(&metrics, &rules_from_map(map))
}

/// Turn a stored dimensions value into the structured object. A string becomes
/// null. An object that is already structured is kept.
pub(crate) fn normalize(method: &str, kind: &str, dimensions: &Value) -> Value {
    let family = family(method, kind);
    let rules = rules_from_value(dimensions);
    let mut metrics = Metrics::default();
    absorb_value(&mut metrics, family, dimensions);
    finish(&mut metrics);
    pack(&metrics, &rules)
}

/// Operator-facing size. A bolt is `M6x30`. A label is `130x37 mm`.
/// When the part carries measurement rules, those fields supply the two
/// numbers. A missing rule uses the part type. Across-flats is omitted from
/// the bolt line. It stays on the part when it was recorded.
pub(crate) fn canonical(method: &str, kind: &str, dimensions: &Value) -> String {
    let family = family(method, kind);
    let metrics = read(family, dimensions);
    let rules = rules_from_value(dimensions);
    if rules.is_empty() {
        return render(family, &metrics);
    }
    render_ruled(family, &metrics, &rules)
}

/// Numeric sizes keyed the way the caliper looks them up (`lengthmm`, `threadmm`).
pub(crate) fn pairs(method: &str, dimensions: &Value) -> Vec<(String, f64)> {
    let metrics = read(family(method, ""), dimensions);
    let mut rows = Vec::new();
    push_pair(&mut rows, "lengthmm", metrics.length_mm);
    push_pair(&mut rows, "threadmm", metrics.thread_mm);
    push_pair(&mut rows, "afmm", metrics.af_mm);
    push_pair(&mut rows, "widthmm", metrics.width_mm);
    push_pair(&mut rows, "heightmm", metrics.height_mm);
    push_pair(&mut rows, "odmm", metrics.od_mm);
    push_pair(&mut rows, "idmm", metrics.id_mm);
    push_pair(&mut rows, "thicknessmm", metrics.thickness_mm);
    rows
}

/// Tolerance rows in a stable order. The key is the caliper name.
pub(crate) fn rows(dimensions: &Value) -> Vec<(&'static str, &'static str, String)> {
    let metrics = read("", dimensions);
    let mut rows = Vec::new();
    push_row(&mut rows, "lengthmm", "length", metrics.length_mm);
    push_row(&mut rows, "threadmm", "thread", metrics.thread_mm);
    push_row(&mut rows, "afmm", "AF", metrics.af_mm);
    push_row(&mut rows, "widthmm", "width", metrics.width_mm);
    push_row(&mut rows, "heightmm", "height", metrics.height_mm);
    push_row(&mut rows, "odmm", "OD", metrics.od_mm);
    push_row(&mut rows, "idmm", "ID", metrics.id_mm);
    push_row(&mut rows, "thicknessmm", "thickness", metrics.thickness_mm);
    rows
}

fn read(family: &str, dimensions: &Value) -> Metrics {
    let mut metrics = Metrics::default();
    absorb_value(&mut metrics, family, dimensions);
    finish(&mut metrics);
    metrics
}

fn family(method: &str, kind: &str) -> &'static str {
    match method {
        "bolt" => return "bolt",
        "nut" => return "nut",
        "washer" => return "washer",
        "cable_tie" => return "cable_tie",
        "spring" => return "spring",
        _ => {}
    }
    let kind = kind.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match kind.as_str() {
        "bolt" => "bolt",
        "nut" => "nut",
        "washer" | "spacer" | "crush_tube" => "washer",
        "label" | "sticker" => "label",
        "cable_tie" | "cabletie" => "cable_tie",
        "spring" => "spring",
        _ => "",
    }
}

fn absorb_top(metrics: &mut Metrics, family: &str, map: &Map<String, Value>) {
    for (key, value) in map {
        if !top_key(key) {
            continue;
        }
        assign(metrics, family, key, value, false);
    }
}

fn top_key(key: &str) -> bool {
    matches!(
        norm(key).as_str(),
        "thread"
            | "threadmm"
            | "threaddiameter"
            | "threaddiametermm"
            | "lengthmm"
            | "lengthunderhead"
            | "afmm"
            | "acrossflats"
            | "widthmm"
            | "strapwidth"
            | "heightmm"
            | "freeheight"
            | "odmm"
            | "outerdiameter"
            | "idmm"
            | "innerdiameter"
            | "thicknessmm"
            | "thickness"
            | "headtype"
            | "length"
            | "width"
            | "height"
            | "diameter"
            | "diametermm"
            | "od"
            | "h"
            | "af"
    )
}

fn absorb_dim_slot(
    metrics: &mut Metrics,
    family: &str,
    map: &Map<String, Value>,
    name_key: &str,
    mm_key: &str,
) {
    let Some(mm) = map.get(mm_key) else {
        return;
    };
    let name = map
        .get(name_key)
        .and_then(Value::as_str)
        .unwrap_or(name_key);
    assign(metrics, family, name, mm, true);
}

fn absorb_value(metrics: &mut Metrics, family: &str, value: &Value) {
    match value {
        Value::Object(map) => {
            if let Some(name) = map.get("name").and_then(Value::as_str) {
                if let Some(mm) = map.get("mm").or_else(|| map.get("value")) {
                    assign(metrics, family, name, mm, true);
                }
            }
            for (key, item) in map {
                if key.eq_ignore_ascii_case("name")
                    || key.eq_ignore_ascii_case("mm")
                    || key.eq_ignore_ascii_case("value")
                {
                    continue;
                }
                assign(metrics, family, key, item, true);
            }
        }
        Value::Array(items) => {
            for item in items {
                absorb_value(metrics, family, item);
            }
        }
        _ => {}
    }
}

fn assign(metrics: &mut Metrics, family: &str, key: &str, value: &Value, from_name: bool) {
    match slot(family, &norm(key), from_name) {
        Slot::Thread => {
            if metrics.thread.is_empty() {
                if let Some(text) = thread_text(value) {
                    metrics.thread = text;
                } else if let Some(mm) = metric_mm(value) {
                    set_mm(&mut metrics.thread_mm, mm);
                }
            }
        }
        Slot::ThreadMm => {
            if let Some(mm) = metric_mm(value) {
                set_mm(&mut metrics.thread_mm, mm);
            }
        }
        Slot::Length => {
            if let Some(mm) = metric_mm(value) {
                set_mm(&mut metrics.length_mm, mm);
            }
        }
        Slot::Af => {
            if let Some(mm) = metric_mm(value) {
                set_mm(&mut metrics.af_mm, mm);
            }
        }
        Slot::Width => {
            if let Some(mm) = metric_mm(value) {
                set_mm(&mut metrics.width_mm, mm);
            }
        }
        Slot::Height => {
            if let Some(mm) = metric_mm(value) {
                set_mm(&mut metrics.height_mm, mm);
            }
        }
        Slot::Od => {
            if let Some(mm) = metric_mm(value) {
                set_mm(&mut metrics.od_mm, mm);
            }
        }
        Slot::Id => {
            if let Some(mm) = metric_mm(value) {
                set_mm(&mut metrics.id_mm, mm);
            }
        }
        Slot::Thickness => {
            if let Some(mm) = metric_mm(value) {
                set_mm(&mut metrics.thickness_mm, mm);
            }
        }
        Slot::Head => {
            if metrics.head_type.is_empty() {
                if let Some(text) = thread_text(value) {
                    metrics.head_type = text;
                }
            }
        }
        Slot::Skip => {}
    }
}

fn set_mm(slot: &mut Option<f64>, mm: f64) {
    if slot.is_none() {
        *slot = Some(mm);
    }
}

fn slot(family: &str, norm: &str, from_name: bool) -> Slot {
    match norm {
        "thread" => return Slot::Thread,
        "threadmm" | "threaddiameter" | "threaddia" | "threaddiametermm" | "nominaldiameter" => {
            return Slot::ThreadMm;
        }
        "lengthmm" | "lengthunderhead" | "underheadlength" | "shanklength" | "griplength" => {
            return Slot::Length;
        }
        "afmm" | "acrossflats" | "acrossflatsaf" | "widthacrossflats" | "flats" => return Slot::Af,
        "widthmm" | "strapwidth" => return Slot::Width,
        "heightmm" => return Slot::Height,
        "odmm" | "outerdiameter" | "outsidediameter" => return Slot::Od,
        "idmm" | "innerdiameter" | "insidediameter" => return Slot::Id,
        "thicknessmm" | "thickness" | "thk" => return Slot::Thickness,
        "headtype" => return Slot::Head,
        _ => {}
    }
    if !from_name
        && !matches!(
            norm,
            "length" | "width" | "height" | "diameter" | "diametermm" | "od" | "h" | "af"
        )
    {
        return Slot::Skip;
    }
    match (family, norm) {
        ("bolt", "length" | "len" | "l") => Slot::Length,
        ("bolt", "diameter" | "dia" | "diametermm") => Slot::ThreadMm,
        ("bolt", "af") => Slot::Af,
        ("nut", "af") => Slot::Af,
        ("nut", "height" | "h") => Slot::Height,
        ("washer", "od" | "diameter" | "dia" | "diametermm") => Slot::Od,
        ("washer", "id") => Slot::Id,
        ("washer", "length" | "len" | "l") => Slot::Length,
        ("label", "width" | "w" | "length" | "len" | "l") => Slot::Width,
        ("label", "height" | "h") => Slot::Height,
        ("cable_tie", "length" | "len" | "l" | "totallength" | "overalllength") => Slot::Length,
        ("cable_tie", "width" | "w" | "bandwidth") => Slot::Width,
        ("spring", "height" | "h" | "length" | "len" | "l" | "freeheight") => Slot::Length,
        ("spring", "diameter" | "dia" | "od" | "coildiameter") => Slot::Od,
        (_, "freeheight") => Slot::Length,
        (_, "length" | "len" | "l") => Slot::Length,
        (_, "height" | "h") => Slot::Height,
        (_, "width" | "w") => Slot::Width,
        (_, "od") => Slot::Od,
        (_, "id") if from_name => Slot::Id,
        (_, "diameter" | "dia" | "diametermm") => Slot::ThreadMm,
        (_, "af") => Slot::Af,
        _ => Slot::Skip,
    }
}

fn finish(metrics: &mut Metrics) {
    if metrics.thread.is_empty() {
        if let Some(mm) = metrics.thread_mm {
            metrics.thread = format!("M{}", compact(mm));
        }
    } else if metrics.thread_mm.is_none() {
        metrics.thread_mm = nominal_thread(&metrics.thread);
    }
}

/// Nominal millimetres from a metric thread code such as `M6` or `M6x1`.
/// Anything else, including a fractional designation, stays unset.
fn nominal_thread(thread: &str) -> Option<f64> {
    let rest = thread.trim().strip_prefix(['M', 'm'])?;
    let (number, end) = leading_number(rest)?;
    let after = rest[end..].trim_start();
    if after.is_empty() || after.starts_with(['x', '×', '-', ' ']) {
        Some(number)
    } else {
        None
    }
}

fn leading_number(text: &str) -> Option<(f64, usize)> {
    let mut end = 0;
    let mut dot = false;
    let mut digits = false;
    for (index, ch) in text.char_indices() {
        if ch.is_ascii_digit() {
            digits = true;
            end = index + 1;
            continue;
        }
        if ch == '.' && !dot && digits {
            dot = true;
            end = index + 1;
            continue;
        }
        break;
    }
    if !digits {
        return None;
    }
    let number: f64 = text[..end].parse().ok()?;
    metric_positive(number).map(|number| (number, end))
}

fn thread_text(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    if text.is_empty() || text.chars().count() > 40 {
        None
    } else {
        Some(text.to_string())
    }
}

fn metric_mm(value: &Value) -> Option<f64> {
    let number = if let Some(number) = value.as_f64() {
        number
    } else if let Some(text) = value.as_str() {
        text.trim().parse().ok()?
    } else {
        return None;
    };
    metric_positive(number)
}

fn metric_positive(number: f64) -> Option<f64> {
    if number.is_finite() && number > 0.0 {
        Some(number)
    } else {
        None
    }
}

fn norm(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn present(metrics: &Metrics) -> bool {
    !metrics.thread.is_empty()
        || metrics.thread_mm.is_some()
        || metrics.length_mm.is_some()
        || metrics.af_mm.is_some()
        || metrics.width_mm.is_some()
        || metrics.height_mm.is_some()
        || metrics.od_mm.is_some()
        || metrics.id_mm.is_some()
        || metrics.thickness_mm.is_some()
        || !metrics.head_type.is_empty()
}

fn to_value(metrics: &Metrics) -> Value {
    if !present(metrics) {
        return Value::Null;
    }
    let mut map = Map::new();
    if !metrics.thread.is_empty() {
        map.insert("thread".to_string(), Value::String(metrics.thread.clone()));
    }
    insert_mm(&mut map, "thread_mm", metrics.thread_mm);
    insert_mm(&mut map, "length_mm", metrics.length_mm);
    insert_mm(&mut map, "af_mm", metrics.af_mm);
    insert_mm(&mut map, "width_mm", metrics.width_mm);
    insert_mm(&mut map, "height_mm", metrics.height_mm);
    insert_mm(&mut map, "od_mm", metrics.od_mm);
    insert_mm(&mut map, "id_mm", metrics.id_mm);
    insert_mm(&mut map, "thickness_mm", metrics.thickness_mm);
    if !metrics.head_type.is_empty() {
        map.insert(
            "head_type".to_string(),
            Value::String(metrics.head_type.clone()),
        );
    }
    Value::Object(map)
}

fn insert_mm(map: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    let Some(value) = value else {
        return;
    };
    if let Some(number) = serde_json::Number::from_f64(value) {
        map.insert(key.to_string(), Value::Number(number));
    }
}

fn insert_text(map: &mut Map<String, Value>, key: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    map.insert(key.to_string(), Value::String(value.to_string()));
}

#[derive(Clone, Debug, Default)]
struct Rules {
    length_rule: String,
    diameter_rule: String,
    note: String,
}

impl Rules {
    fn is_empty(&self) -> bool {
        self.length_rule.is_empty() && self.diameter_rule.is_empty()
    }
}

/// One ruled axis. `Unset` means the catalogue has no rule, so the caller
/// keeps its previous field list. `Missing` means the rule names a field that
/// has no number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RuleSide {
    Unset,
    Missing,
    Ready(f64),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MeasurementRules {
    pub length: RuleSide,
    pub diameter: RuleSide,
    pub length_key: Option<&'static str>,
    pub diameter_key: Option<&'static str>,
}

/// Length and diameter selected by the stored class rules.
pub(crate) fn measurement_rules(dimensions: &Value) -> MeasurementRules {
    let metrics = read("", dimensions);
    let rules = rules_from_value(dimensions);
    let length = rule_side(&metrics, &rules.length_rule);
    let diameter = rule_side(&metrics, &rules.diameter_rule);
    MeasurementRules {
        length: length.0,
        diameter: diameter.0,
        length_key: length.1,
        diameter_key: diameter.1,
    }
}

fn pack(metrics: &Metrics, rules: &Rules) -> Value {
    let mut value = to_value(metrics);
    if let Some(map) = value.as_object_mut() {
        insert_text(map, "length_rule", &rules.length_rule);
        insert_text(map, "diameter_rule", &rules.diameter_rule);
        insert_text(map, "rule_note", &rules.note);
    }
    value
}

fn rules_from_map(map: &Map<String, Value>) -> Rules {
    let nested = map.get("measurement_rules").and_then(Value::as_object);
    Rules {
        length_rule: rule_text(nested, map, "length_rule"),
        diameter_rule: rule_text(nested, map, "diameter_rule"),
        note: note_text(nested, map),
    }
}

fn rules_from_value(value: &Value) -> Rules {
    let Some(map) = value.as_object() else {
        return Rules::default();
    };
    rules_from_map(map)
}

fn rule_text(nested: Option<&Map<String, Value>>, map: &Map<String, Value>, key: &str) -> String {
    nested
        .and_then(|rules| clean_rule(rules.get(key)))
        .or_else(|| clean_rule(map.get(key)))
        .unwrap_or_default()
}

fn note_text(nested: Option<&Map<String, Value>>, map: &Map<String, Value>) -> String {
    nested
        .and_then(|rules| clean_rule(rules.get("note")))
        .or_else(|| clean_rule(map.get("rule_note")))
        .or_else(|| nested.and_then(|rules| clean_rule(rules.get("rule_note"))))
        .unwrap_or_default()
}

fn clean_rule(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    if text.is_empty() || text.chars().count() > 200 {
        None
    } else {
        Some(text.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Field {
    Length,
    Height,
    Thickness,
    Width,
    Od,
    Id,
    Af,
    Thread,
}

fn rule_side(metrics: &Metrics, rule: &str) -> (RuleSide, Option<&'static str>) {
    let Some(field) = field_for_rule(rule) else {
        return (RuleSide::Unset, None);
    };
    match field_mm(metrics, field) {
        Some(mm) => (RuleSide::Ready(mm), Some(field_key(field))),
        None => (RuleSide::Missing, Some(field_key(field))),
    }
}

/// Empty and unknown rules are unset, so the part type can fill that side.
/// A known field with no number stays missing.
fn field_for_rule(rule: &str) -> Option<Field> {
    if rule.trim().is_empty() {
        return None;
    }
    Some(match norm(rule).as_str() {
        "lengthmm" | "length" | "len" | "l" | "lengthunderhead" | "shanklength"
        | "underheadlength" | "griplength" | "shank" => Field::Length,
        "heightmm" | "height" | "h" | "freeheight" => Field::Height,
        "thicknessmm" | "thickness" | "thk" => Field::Thickness,
        "widthmm" | "width" | "w" | "strapwidth" | "bandwidth" => Field::Width,
        "odmm" | "od" | "outerdiameter" | "outsidediameter" => Field::Od,
        "idmm" | "id" | "innerdiameter" | "insidediameter" => Field::Id,
        "afmm" | "af" | "acrossflats" | "acrossflatsaf" | "widthacrossflats" | "flats" => Field::Af,
        "thread" | "threadmm" | "threaddiameter" | "threaddia" | "nominaldiameter" => Field::Thread,
        _ => return None,
    })
}

fn field_mm(metrics: &Metrics, field: Field) -> Option<f64> {
    match field {
        Field::Length => metrics.length_mm,
        Field::Height => metrics.height_mm,
        Field::Thickness => metrics.thickness_mm,
        Field::Width => metrics.width_mm,
        Field::Od => metrics.od_mm,
        Field::Id => metrics.id_mm,
        Field::Af => metrics.af_mm,
        Field::Thread => metrics.thread_mm,
    }
}

fn field_key(field: Field) -> &'static str {
    match field {
        Field::Length => "lengthmm",
        Field::Height => "heightmm",
        Field::Thickness => "thicknessmm",
        Field::Width => "widthmm",
        Field::Od => "odmm",
        Field::Id => "idmm",
        Field::Af => "afmm",
        Field::Thread => "threadmm",
    }
}

struct Axis {
    field: Field,
    mm: Option<f64>,
}

fn render_ruled(family: &str, metrics: &Metrics, rules: &Rules) -> String {
    let length = display_axis(metrics, family, &rules.length_rule, true);
    let diameter = display_axis(metrics, family, &rules.diameter_rule, false);
    match (length, diameter) {
        (Some(length), Some(diameter)) => pair_text(metrics, length, diameter),
        (None, Some(diameter)) if diameter.field == Field::Thread => bolt_line(metrics, None),
        (Some(length), None) => one_text(length),
        (None, Some(diameter)) => one_text(diameter),
        (None, None) => String::new(),
    }
}

fn display_axis(metrics: &Metrics, family: &str, rule: &str, length: bool) -> Option<Axis> {
    if let Some(field) = field_for_rule(rule) {
        return Some(Axis {
            field,
            mm: field_mm(metrics, field),
        })
        .filter(|axis| axis.mm.is_some() || field == Field::Thread && !thread_token(metrics).is_empty());
    }
    fallback_axis(metrics, family, length)
}

fn fallback_axis(metrics: &Metrics, family: &str, length: bool) -> Option<Axis> {
    let field = if length {
        match family {
            "nut" | "label" => Field::Height,
            "washer" => Field::Thickness,
            _ => {
                if metrics.length_mm.is_some() {
                    Field::Length
                } else if metrics.height_mm.is_some() {
                    Field::Height
                } else {
                    return None;
                }
            }
        }
    } else {
        match family {
            "nut" => Field::Af,
            "label" | "cable_tie" => Field::Width,
            "washer" | "spring" => Field::Od,
            "bolt" => Field::Thread,
            _ => {
                if metrics.thread_mm.is_some() || !metrics.thread.is_empty() {
                    Field::Thread
                } else if metrics.od_mm.is_some() {
                    Field::Od
                } else if metrics.af_mm.is_some() {
                    Field::Af
                } else if metrics.width_mm.is_some() {
                    Field::Width
                } else {
                    return None;
                }
            }
        }
    };
    let axis = Axis {
        field,
        mm: field_mm(metrics, field),
    };
    if axis.mm.is_some() || field == Field::Thread && !thread_token(metrics).is_empty() {
        Some(axis)
    } else {
        None
    }
}

fn pair_text(metrics: &Metrics, length: Axis, diameter: Axis) -> String {
    match (diameter.field, length.field) {
        (Field::Thread, _) => bolt_line(metrics, length.mm),
        (Field::Af, Field::Height) => nut_line(metrics, diameter.mm, length.mm),
        (Field::Af, _) => append_named(nut_line(metrics, diameter.mm, None), "L", length.mm),
        (Field::Width, Field::Height) => mm_pair(diameter.mm, length.mm),
        (Field::Width, Field::Length) | (Field::Length, Field::Width) => {
            mm_pair(length.mm, diameter.mm)
        }
        (Field::Od, Field::Thickness) => washer_line(metrics, diameter.mm, length.mm),
        (Field::Od, Field::Length) => named_pair("OD", diameter.mm, "L", length.mm),
        (Field::Od, Field::Height) => named_pair("OD", diameter.mm, "H", length.mm),
        _ => mm_pair(diameter.mm, length.mm),
    }
}

fn one_text(axis: Axis) -> String {
    match axis.field {
        Field::Thread => axis
            .mm
            .map(|mm| format!("M{}", compact(mm)))
            .unwrap_or_default(),
        Field::Af => axis
            .mm
            .map(|mm| format!("AF: {}mm", compact(mm)))
            .unwrap_or_default(),
        Field::Od => axis
            .mm
            .map(|mm| format!("OD: {}mm", compact(mm)))
            .unwrap_or_default(),
        Field::Id => axis
            .mm
            .map(|mm| format!("ID: {}mm", compact(mm)))
            .unwrap_or_default(),
        Field::Thickness => axis
            .mm
            .map(|mm| format!("{} mm", compact(mm)))
            .unwrap_or_default(),
        _ => axis
            .mm
            .map(|mm| format!("{} mm", compact(mm)))
            .unwrap_or_default(),
    }
}

fn bolt_line(metrics: &Metrics, length: Option<f64>) -> String {
    let thread = thread_token(metrics);
    match (thread.is_empty(), length) {
        (false, Some(length)) => format!("{thread}x{}", compact(length)),
        (false, None) => thread,
        (true, Some(length)) => format!("{} mm", compact(length)),
        (true, None) => String::new(),
    }
}

fn nut_line(metrics: &Metrics, af: Option<f64>, height: Option<f64>) -> String {
    let mut text = String::new();
    if !metrics.thread.is_empty() {
        text = metrics.thread.clone();
    }
    if let Some(af) = af {
        let af = format!("AF: {}mm", compact(af));
        text = if text.is_empty() {
            af
        } else {
            format!("{text}, {af}")
        };
    }
    if let Some(height) = height {
        let height = format!("H: {}mm", compact(height));
        text = if text.is_empty() {
            height
        } else {
            format!("{text}, {height}")
        };
    }
    text
}

fn append_named(text: String, label: &str, mm: Option<f64>) -> String {
    let Some(mm) = mm else {
        return text;
    };
    let extra = format!("{label}: {}mm", compact(mm));
    if text.is_empty() {
        extra
    } else {
        format!("{text}, {extra}")
    }
}

fn washer_line(metrics: &Metrics, od: Option<f64>, thickness: Option<f64>) -> String {
    match (od, metrics.id_mm, thickness) {
        (Some(od), Some(id), Some(thickness)) => format!(
            "{} × {} × {} mm",
            compact(od),
            compact(id),
            compact(thickness)
        ),
        (Some(od), None, Some(thickness)) => {
            format!("{} × {} mm", compact(od), compact(thickness))
        }
        (Some(od), Some(id), None) => format!("{} × {} mm", compact(od), compact(id)),
        (Some(od), None, None) => format!("OD: {}mm", compact(od)),
        _ => String::new(),
    }
}

fn named_pair(left: &str, left_mm: Option<f64>, right: &str, right_mm: Option<f64>) -> String {
    match (left_mm, right_mm) {
        (Some(left_mm), Some(right_mm)) => {
            format!("{left}: {}mm, {right}: {}mm", compact(left_mm), compact(right_mm))
        }
        (Some(left_mm), None) => format!("{left}: {}mm", compact(left_mm)),
        (None, Some(right_mm)) => format!("{right}: {}mm", compact(right_mm)),
        (None, None) => String::new(),
    }
}

fn mm_pair(left: Option<f64>, right: Option<f64>) -> String {
    match (left, right) {
        (Some(left), Some(right)) => format!("{}x{} mm", compact(left), compact(right)),
        (Some(left), None) => format!("{} mm", compact(left)),
        (None, Some(right)) => format!("{} mm", compact(right)),
        (None, None) => String::new(),
    }
}

fn render(family: &str, metrics: &Metrics) -> String {
    match family {
        "bolt" => bolt_size(metrics),
        "nut" => nut_size(metrics),
        "washer" => washer_size(metrics),
        "label" => label_size(metrics),
        "cable_tie" => pair_size(metrics.length_mm, metrics.width_mm),
        "spring" => spring_size(metrics),
        _ => generic_size(metrics),
    }
}

fn bolt_size(metrics: &Metrics) -> String {
    let thread = thread_token(metrics);
    match (thread.is_empty(), metrics.length_mm) {
        (false, Some(length)) => format!("{thread}x{}", compact(length)),
        (false, None) => thread,
        (true, Some(length)) => format!("{} mm", compact(length)),
        _ => String::new(),
    }
}

fn thread_token(metrics: &Metrics) -> String {
    if !metrics.thread.is_empty() {
        return metrics.thread.clone();
    }
    metrics
        .thread_mm
        .map(|mm| format!("M{}", compact(mm)))
        .unwrap_or_default()
}

fn nut_size(metrics: &Metrics) -> String {
    let mut text = String::new();
    if !metrics.thread.is_empty() {
        if let Some(af) = metrics.af_mm {
            text = format!("{}, AF: {}mm", metrics.thread, compact(af));
        } else {
            text = metrics.thread.clone();
        }
    } else if let Some(af) = metrics.af_mm {
        text = format!("AF: {}mm", compact(af));
    }
    if let Some(height) = metrics.height_mm {
        let height = format!("H: {}mm", compact(height));
        if text.is_empty() {
            text = height;
        } else {
            text = format!("{text}, {height}");
        }
    }
    text
}

fn washer_size(metrics: &Metrics) -> String {
    match (metrics.od_mm, metrics.id_mm, metrics.thickness_mm) {
        (Some(od), Some(id), Some(thickness)) => format!(
            "{} × {} × {} mm",
            compact(od),
            compact(id),
            compact(thickness)
        ),
        (Some(od), None, Some(thickness)) => {
            format!("{} × {} mm", compact(od), compact(thickness))
        }
        (Some(od), Some(id), None) => format!("{} × {} mm", compact(od), compact(id)),
        (Some(od), None, None) => match metrics.length_mm {
            Some(length) => format!("OD: {}mm, L: {}mm", compact(od), compact(length)),
            None => format!("OD: {}mm", compact(od)),
        },
        _ => generic_size(metrics),
    }
}

fn label_size(metrics: &Metrics) -> String {
    match (metrics.width_mm, metrics.height_mm) {
        (Some(width), Some(height)) => format!("{}x{} mm", compact(width), compact(height)),
        _ => generic_size(metrics),
    }
}

fn pair_size(length: Option<f64>, width: Option<f64>) -> String {
    match (length, width) {
        (Some(length), Some(width)) => format!("{}x{} mm", compact(length), compact(width)),
        (Some(length), None) => format!("{} mm", compact(length)),
        (None, Some(width)) => format!("{} mm", compact(width)),
        _ => String::new(),
    }
}

fn spring_size(metrics: &Metrics) -> String {
    match (metrics.od_mm, metrics.length_mm) {
        (Some(od), Some(length)) => format!("OD: {}mm, L: {}mm", compact(od), compact(length)),
        _ => generic_size(metrics),
    }
}

fn generic_size(metrics: &Metrics) -> String {
    let thread = thread_token(metrics);
    if !thread.is_empty() {
        if let Some(length) = metrics.length_mm {
            return format!("{thread}x{}", compact(length));
        }
    }
    if let (Some(od), Some(length)) = (metrics.od_mm, metrics.length_mm) {
        return format!("OD: {}mm, L: {}mm", compact(od), compact(length));
    }
    if let (Some(width), Some(height)) = (metrics.width_mm, metrics.height_mm) {
        return format!("{}x{} mm", compact(width), compact(height));
    }
    if let (Some(width), Some(length)) = (metrics.width_mm, metrics.length_mm) {
        return format!("{}x{} mm", compact(width), compact(length));
    }
    if let (Some(od), Some(thickness)) = (metrics.od_mm, metrics.thickness_mm) {
        return format!("OD: {}mm, Thk: {}mm", compact(od), compact(thickness));
    }
    if let (Some(od), Some(height)) = (metrics.od_mm, metrics.height_mm) {
        return format!("OD: {}mm, H: {}mm", compact(od), compact(height));
    }
    if let Some(length) = metrics.length_mm {
        return format!("{} mm", compact(length));
    }
    if let Some(od) = metrics.od_mm {
        return format!("OD: {}mm", compact(od));
    }
    let nut = nut_size(metrics);
    if !nut.is_empty() {
        return nut;
    }
    String::new()
}

fn push_pair(rows: &mut Vec<(String, f64)>, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        rows.push((key.to_string(), value));
    }
}

fn push_row(
    rows: &mut Vec<(&'static str, &'static str, String)>,
    key: &'static str,
    label: &'static str,
    value: Option<f64>,
) {
    if let Some(value) = value {
        rows.push((key, label, compact(value)));
    }
}

fn compact(value: f64) -> String {
    let rounded = (value * 1000.0).round() / 1000.0;
    if (rounded - rounded.round()).abs() < 1e-6 {
        return format!("{}", rounded.round() as i64);
    }
    let text = format!("{rounded:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_bolt_is_thread_by_length_and_af_is_optional() {
        let map = json!({
            "ic": "2BB08C2HZ",
            "type": "bolt",
            "thread": "M6",
            "length_mm": 30,
            "af_mm": 10,
            "dimensions": "M6x30 hex"
        });
        let map = map.as_object().unwrap();
        let stored = from_part("bolt", "bolt", map);
        assert_eq!(stored["thread"], "M6");
        assert_eq!(stored["length_mm"], 30.0);
        assert_eq!(stored["thread_mm"], 6.0);
        assert_eq!(stored["af_mm"], 10.0);
        assert_eq!(canonical("bolt", "bolt", &stored), "M6x30");
        let pairs = pairs("bolt", &stored);
        assert!(pairs.iter().any(|(key, value)| key == "afmm" && *value == 10.0));
        assert!(pairs.iter().any(|(key, value)| key == "lengthmm" && *value == 30.0));
    }

    #[test]
    fn a_bolt_without_af_still_has_a_size() {
        let stored = from_part(
            "",
            "bolt",
            json!({
                "dimensions": [
                    { "name": "Length under head", "mm": 16 },
                    { "name": "Thread diameter", "mm": 6 }
                ]
            })
            .as_object()
            .unwrap(),
        );
        assert!(stored.get("af_mm").is_none());
        assert_eq!(canonical("", "bolt", &stored), "M6x16");
    }

    #[test]
    fn a_label_is_width_by_height() {
        let stored = from_part(
            "",
            "label",
            json!({
                "width_mm": 130,
                "height_mm": 37,
                "dimensions": "130 x 37 caution"
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(canonical("", "label", &stored), "130x37 mm");
        let from_names = from_part(
            "",
            "label",
            json!({
                "dimensions": [{ "name": "L", "mm": 130 }, { "name": "H", "mm": 37 }]
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(canonical("", "label", &from_names), "130x37 mm");
    }

    #[test]
    fn a_dimensions_string_and_a_description_do_not_become_numbers() {
        let stored = from_part(
            "",
            "bolt",
            json!({
                "description": "M6x40 bolt",
                "dimensions": "M6x40"
            })
            .as_object()
            .unwrap(),
        );
        assert!(stored.is_null());
        assert!(canonical("", "bolt", &json!("M6x40")).is_empty());
        assert!(pairs("", &json!("M8x20")).is_empty());
    }

    #[test]
    fn a_fractional_thread_is_kept_and_not_turned_into_millimetres() {
        let stored = from_part(
            "nut",
            "nut",
            json!({ "thread": "3/8 UNC", "af_mm": 14.3 })
                .as_object()
                .unwrap(),
        );
        assert_eq!(stored["thread"], "3/8 UNC");
        assert!(stored.get("thread_mm").is_none());
        assert_eq!(canonical("nut", "nut", &stored), "3/8 UNC, AF: 14.3mm");
    }

    #[test]
    fn imported_name_lists_keep_the_marker_size() {
        let bolt = from_part(
            "",
            "bolt",
            json!({
                "dimensions": [
                    { "name": "Length under head", "mm": 20 },
                    { "name": "Thread diameter", "mm": 8 }
                ]
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(canonical("", "bolt", &bolt), "M8x20");
        let label = from_part(
            "",
            "label",
            json!({
                "dimensions": [{ "name": "l", "mm": 130 }, { "name": "h", "mm": 37 }]
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(canonical("", "label", &label), "130x37 mm");
        let nut = from_part(
            "",
            "nut",
            json!({
                "dimensions": [
                    { "name": "Across flats (A/F)", "mm": 13 },
                    { "name": "Height", "mm": 7.85 }
                ]
            })
            .as_object()
            .unwrap(),
        );
        assert!(nut.get("thread_mm").is_none());
        assert_eq!(canonical("", "nut", &nut), "AF: 13mm, H: 7.85mm");
        let spring = from_part(
            "",
            "spring",
            json!({
                "dimensions": [
                    { "name": "Height", "mm": 13 },
                    { "name": "Diameter", "mm": 15 }
                ]
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(canonical("", "spring", &spring), "OD: 15mm, L: 13mm");
    }

    #[test]
    fn a_marker_export_uses_the_dim_slots_and_ignores_the_display_string() {
        let bolt = from_part(
            "",
            "bolt",
            json!({
                "part_type": "bolt",
                "description": "hex bolt",
                "dimensions": [
                    { "name": "Length under head", "mm": 25 },
                    { "name": "Thread diameter", "mm": 8 }
                ],
                "length_rule": "length_mm",
                "diameter_rule": "thread",
                "dim1_name": "Length under head",
                "dim1_mm": 25,
                "dim2_name": "Thread diameter",
                "dim2_mm": 8
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(bolt["thread"], "M8");
        assert_eq!(bolt["thread_mm"], 8.0);
        assert_eq!(bolt["length_mm"], 25.0);
        assert!(bolt.get("af_mm").is_none());
        assert_eq!(canonical("", "bolt", &bolt), "M8x25");

        let label = from_part(
            "",
            "label",
            json!({
                "dimensions": "130x37mm",
                "dim1_name": "Width",
                "dim1_mm": 130,
                "dim2_name": "Height",
                "dim2_mm": 37,
                "length_rule": "height_mm",
                "diameter_rule": "width_mm"
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(label["width_mm"], 130.0);
        assert_eq!(label["height_mm"], 37.0);
        assert_eq!(canonical("", "label", &label), "130x37 mm");

        let loaded = normalize(
            "",
            "label",
            &json!([{ "name": "L", "mm": 130 }, { "name": "H", "mm": 37 }]),
        );
        assert_eq!(canonical("", "label", &loaded), "130x37 mm");

        let cable = from_part(
            "",
            "cable_tie",
            json!({
                "dimensions": [
                    { "name": "Length", "mm": 150 },
                    { "name": "Width", "mm": 3.7 }
                ]
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(canonical("", "cable_tie", &cable), "150x3.7 mm");

        let tube = from_part(
            "",
            "washer",
            json!({
                "dimensions": [{ "name": "l", "mm": 32 }, { "name": "OD", "mm": 12 }]
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(canonical("", "washer", &tube), "OD: 12mm, L: 32mm");
    }

    #[test]
    fn measurement_rules_select_the_length_and_the_diameter() {
        let bolt = from_part(
            "",
            "bolt",
            json!({
                "thread": "M6",
                "length_mm": 30,
                "af_mm": 10,
                "dimensions": "M6x30 hex",
                "measurement_rules": {
                    "length_rule": "length_mm",
                    "diameter_rule": "thread",
                    "note": "Shank length and nominal thread diameter"
                }
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(bolt["length_rule"], "length_mm");
        assert_eq!(bolt["diameter_rule"], "thread");
        assert_eq!(bolt["rule_note"], "Shank length and nominal thread diameter");
        assert_eq!(canonical("", "bolt", &bolt), "M6x30");
        let rules = measurement_rules(&bolt);
        assert_eq!(rules.length, RuleSide::Ready(30.0));
        assert_eq!(rules.diameter, RuleSide::Ready(6.0));
        assert_eq!(rules.diameter_key, Some("threadmm"));
        let again = normalize("", "bolt", &bolt);
        assert_eq!(again["length_rule"], "length_mm");
        assert_eq!(canonical("", "bolt", &again), "M6x30");

        let nut = from_part(
            "",
            "nut",
            json!({
                "af_mm": 13,
                "height_mm": 7.85,
                "od_mm": 99,
                "length_rule": "height_mm",
                "diameter_rule": "af_mm"
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(canonical("", "nut", &nut), "AF: 13mm, H: 7.85mm");
        let nut_rules = measurement_rules(&nut);
        assert_eq!(nut_rules.diameter, RuleSide::Ready(13.0));
        assert_eq!(nut_rules.length, RuleSide::Ready(7.85));

        let missing = from_part(
            "",
            "bolt",
            json!({
                "thread": "M8",
                "height_mm": 4,
                "length_rule": "length_mm",
                "diameter_rule": "thread"
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(canonical("", "bolt", &missing), "M8");
        assert_eq!(measurement_rules(&missing).length, RuleSide::Missing);

        let label = from_part(
            "",
            "other",
            json!({
                "width_mm": 130,
                "height_mm": 37,
                "length_mm": 999,
                "length_rule": "height_mm",
                "diameter_rule": "width_mm"
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(canonical("", "other", &label), "130x37 mm");
    }
}
