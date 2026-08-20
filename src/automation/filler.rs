//! Working out what belongs in each box on an opt-out form.
//!
//! Ported from `internal/browser/filler.go`, with the matching separated from
//! the browser. Go's version issued a CDP call per candidate selector, so
//! none of the logic that decides *what goes where* could be tested without a
//! running Chrome. Here that decision is a pure function over the fields a
//! page declares, and [`super::browser`] does nothing but read fields out of
//! a page and type into them.

use serde::{Deserialize, Serialize};

use crate::config::Profile;

/// A piece of the profile that a form might ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Email,
    FirstName,
    LastName,
    /// One box for the whole name, rather than two.
    FullName,
    Phone,
    Address,
    City,
    State,
    ZipCode,
    Country,
    DateOfBirth,
}

impl FieldKind {
    /// Every kind, in the order they are tried.
    pub const ALL: &'static [Self] = &[
        Self::Email,
        Self::FirstName,
        Self::LastName,
        Self::FullName,
        Self::Phone,
        Self::Address,
        Self::City,
        Self::State,
        Self::ZipCode,
        Self::Country,
        Self::DateOfBirth,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::FirstName => "first name",
            Self::LastName => "last name",
            Self::FullName => "full name",
            Self::Phone => "phone",
            Self::Address => "address",
            Self::City => "city",
            Self::State => "state",
            Self::ZipCode => "postal code",
            Self::Country => "country",
            Self::DateOfBirth => "date of birth",
        }
    }

    /// The profile value for this kind, empty when it was never filled in.
    pub fn value_from(self, profile: &Profile) -> String {
        match self {
            Self::Email => profile.email.clone(),
            Self::FirstName => profile.first_name.clone(),
            Self::LastName => profile.last_name.clone(),
            Self::FullName => profile.full_name().trim().to_string(),
            Self::Phone => profile.phone.clone(),
            Self::Address => profile.address.clone(),
            Self::City => profile.city.clone(),
            Self::State => profile.state.clone(),
            Self::ZipCode => profile.zip_code.clone(),
            Self::Country => profile.country.clone(),
            Self::DateOfBirth => profile.date_of_birth.clone(),
        }
    }

    /// The `autocomplete` value a well-built form would use.
    ///
    /// This is the strongest signal there is: it is the page telling you
    /// outright what the box is for, rather than a name that has to be
    /// guessed at.
    const fn autocomplete(self) -> &'static [&'static str] {
        match self {
            Self::Email => &["email"],
            Self::FirstName => &["given-name"],
            Self::LastName => &["family-name"],
            Self::FullName => &["name"],
            Self::Phone => &["tel", "tel-national"],
            Self::Address => &["street-address", "address-line1"],
            Self::City => &["address-level2"],
            Self::State => &["address-level1"],
            Self::ZipCode => &["postal-code"],
            Self::Country => &["country", "country-name"],
            Self::DateOfBirth => &["bday"],
        }
    }

    /// Names and ids that mean this and nothing else.
    const fn exact(self) -> &'static [&'static str] {
        match self {
            Self::Email => &["email", "emailaddress", "email_address", "e-mail", "e_mail"],
            Self::FirstName => &["firstname", "first_name", "fname", "first"],
            Self::LastName => &["lastname", "last_name", "lname", "last", "surname"],
            Self::FullName => &["name", "fullname", "full_name", "yourname", "your_name"],
            Self::Phone => &[
                "phone",
                "telephone",
                "tel",
                "mobile",
                "phonenumber",
                "phone_number",
            ],
            Self::Address => &[
                "address",
                "street",
                "streetaddress",
                "street_address",
                "addr",
                "address1",
                "address_1",
            ],
            Self::City => &["city", "town", "locality"],
            Self::State => &["state", "province", "region"],
            Self::ZipCode => &[
                "zip",
                "zipcode",
                "zip_code",
                "postal",
                "postalcode",
                "postal_code",
                "postcode",
            ],
            Self::Country => &["country", "nation"],
            Self::DateOfBirth => &[
                "dob",
                "dateofbirth",
                "date_of_birth",
                "birthdate",
                "birth_date",
                "bday",
                "birthday",
            ],
        }
    }

    /// Fragments that suggest this kind without settling it.
    const fn fuzzy(self) -> &'static [&'static str] {
        match self {
            Self::Email => &["email", "e-mail"],
            Self::FirstName => &["first", "fname", "given"],
            Self::LastName => &["last", "lname", "family", "surname"],
            Self::FullName => &["fullname", "full name", "your name"],
            Self::Phone => &["phone", "tel", "mobile", "cell"],
            Self::Address => &["address", "street", "addr"],
            Self::City => &["city", "town", "locality"],
            Self::State => &["state", "province", "region"],
            Self::ZipCode => &["zip", "postal", "postcode"],
            Self::Country => &["country", "nation"],
            Self::DateOfBirth => &["dob", "birth", "bday"],
        }
    }

    /// The `type` attribute this kind expects, when it implies one.
    const fn input_type(self) -> Option<&'static str> {
        match self {
            Self::Email => Some("email"),
            Self::Phone => Some("tel"),
            Self::DateOfBirth => Some("date"),
            _ => None,
        }
    }
}

impl std::fmt::Display for FieldKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One input on a page, as the page describes it.
///
/// Every field defaults, so a page that reports fewer keys than expected
/// still yields a usable form rather than losing the whole thing to a
/// deserialization error.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FormField {
    /// A selector that reaches this field again.
    pub selector: String,
    pub name: String,
    pub id: String,
    pub placeholder: String,
    /// The `type` attribute: `text`, `email`, `tel`, and so on.
    pub input_type: String,
    pub autocomplete: String,
    /// The visible label, where one could be found.
    pub label: String,
    pub required: bool,
}

impl FormField {
    /// Everything the page says about this field, lowercased.
    fn haystacks(&self) -> [String; 4] {
        [
            self.name.to_lowercase(),
            self.id.to_lowercase(),
            self.placeholder.to_lowercase(),
            self.label.to_lowercase(),
        ]
    }

    /// Whether this is a field worth typing into at all.
    pub fn is_fillable(&self) -> bool {
        !matches!(
            self.input_type.to_lowercase().as_str(),
            "hidden"
                | "submit"
                | "button"
                | "reset"
                | "image"
                | "file"
                | "password"
                | "checkbox"
                | "radio"
        )
    }
}

/// Scores, highest first. An `autocomplete` attribute is the page stating
/// outright what a box is for; an exact name is nearly as good; a fuzzy
/// fragment is a guess.
const SCORE_AUTOCOMPLETE: i32 = 100;
const SCORE_INPUT_TYPE: i32 = 60;
const SCORE_EXACT: i32 = 40;
const SCORE_FUZZY: i32 = 10;
/// Below this, a guess is not worth acting on.
const MINIMUM_SCORE: i32 = 10;

/// How well one field matches one kind.
pub fn score(field: &FormField, kind: FieldKind) -> i32 {
    if !field.is_fillable() {
        return 0;
    }

    let mut score = 0;

    let autocomplete = field.autocomplete.to_lowercase();
    if !autocomplete.is_empty() && kind.autocomplete().contains(&autocomplete.as_str()) {
        score += SCORE_AUTOCOMPLETE;
    }

    let input_type = field.input_type.to_lowercase();
    if kind.input_type() == Some(input_type.as_str()) {
        score += SCORE_INPUT_TYPE;
    }

    let haystacks = field.haystacks();

    // An exact name or id, ignoring separators, means this and nothing else.
    let normalized: Vec<String> = haystacks.iter().map(|text| normalize(text)).collect();
    if kind
        .exact()
        .iter()
        .any(|exact| normalized.iter().any(|text| text == &normalize(exact)))
    {
        score += SCORE_EXACT;
    }

    if kind
        .fuzzy()
        .iter()
        .any(|fragment| haystacks.iter().any(|text| text.contains(fragment)))
    {
        score += SCORE_FUZZY;
    }

    score
}

/// Strip separators so `first_name`, `first-name`, and `firstName` compare
/// equal.
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// One box, and what to type in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFill {
    pub field: FormField,
    pub kind: FieldKind,
    pub value: String,
    /// The score it was matched on, for anyone debugging a wrong fill.
    pub score: i32,
}

/// What a fill will do, worked out before anything is typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FillPlan {
    pub fills: Vec<PlannedFill>,
    /// Fields the form asks for that the profile has no answer to.
    pub unanswered: Vec<FormField>,
    /// Fields nothing was recognised in.
    pub unrecognized: Vec<FormField>,
}

impl FillPlan {
    pub fn is_empty(&self) -> bool {
        self.fills.is_empty()
    }

    /// Whether a required field will be left blank, which usually means the
    /// form will refuse to submit.
    pub fn has_unanswered_required(&self) -> bool {
        self.unanswered.iter().any(|field| field.required)
    }
}

/// Decide what goes in each box.
///
/// Every field is matched against every kind and the best pairing wins, so a
/// field is filled once and a kind fills at most one field. Go tried each
/// kind against a list of selectors independently, which meant a box named
/// `email_address` matched both the email mapping and the address mapping —
/// and the second one overwrote the first with a street address.
pub fn plan(profile: &Profile, fields: &[FormField]) -> FillPlan {
    // Every (score, field, kind) worth considering.
    let mut candidates: Vec<(i32, usize, FieldKind)> = Vec::new();
    for (index, field) in fields.iter().enumerate() {
        for kind in FieldKind::ALL {
            let score = score(field, *kind);
            if score >= MINIMUM_SCORE {
                candidates.push((score, index, *kind));
            }
        }
    }

    // Best first. Ties break on field order then kind order, so the same page
    // always produces the same plan.
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    let mut plan = FillPlan::default();
    let mut used_fields = vec![false; fields.len()];
    let mut used_kinds = std::collections::HashSet::new();
    let mut unanswered_kinds = Vec::new();

    for (score, index, kind) in candidates {
        if used_fields[index] || used_kinds.contains(&kind) {
            continue;
        }

        let value = kind.value_from(profile);
        if value.is_empty() {
            // The form wants it and the profile has no answer. Claim the
            // field anyway, so a weaker guess does not put the wrong thing
            // in it.
            used_fields[index] = true;
            unanswered_kinds.push(index);
            continue;
        }

        used_fields[index] = true;
        used_kinds.insert(kind);
        plan.fills.push(PlannedFill {
            field: fields[index].clone(),
            kind,
            value,
            score,
        });
    }

    // Report in page order, which is the order a person reads the form in.
    plan.fills.sort_by_key(|fill| {
        fields
            .iter()
            .position(|field| field.selector == fill.field.selector)
            .unwrap_or(usize::MAX)
    });

    for index in unanswered_kinds {
        plan.unanswered.push(fields[index].clone());
    }
    for (index, field) in fields.iter().enumerate() {
        if !used_fields[index] && field.is_fillable() {
            plan.unrecognized.push(field.clone());
        }
    }

    plan
}

#[cfg(test)]
mod tests;
