//! KiCad's text variables (`${NAME}`) in field values and paths.

use crate::Error;

/// Replaces every `${NAME}` in `text` with what `lookup` returns for `NAME`; `None` leaves the
/// variable as it is (as KiCad does with variables it cannot resolve).
///
/// # Errors
/// A variable without its closing `}`, or the error of `lookup`.
pub fn expand(
    text: &str,
    mut lookup: impl FnMut(&str) -> Result<Option<String>, Error>,
) -> Result<String, Error> {
    let mut expanded = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        expanded.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| Error::new(format!("unterminated variable in {text:?}")))?;
        let variable = &rest[start..start + end + 1];
        match lookup(&variable[2..variable.len() - 1])? {
            Some(value) => expanded.push_str(&value),
            None => expanded.push_str(variable),
        }
        rest = &rest[start + end + 1..];
    }
    expanded.push_str(rest);
    Ok(expanded)
}

/// The variables of the simulation fields, which KiCad expands to an empty string when the
/// symbol does not have the field.
const SIM_VARIABLES: [&str; 6] = [
    "SIM.DEVICE",
    "SIM.TYPE",
    "SIM.PINS",
    "SIM.PARAMS",
    "SIM.LIBRARY",
    "SIM.NAME",
];

/// Expands the text variables of a symbol's field (e.g. `Value` of `${SIM.PARAMS}`) as KiCad does
/// from the other fields of the symbol: a variable names a field, ignoring case, and the variables
/// of missing simulation fields are empty. Other variables are left as they are, including those
/// KiCad resolves from the schematic (`${REFERENCE}`, `${UNIT}`, sheet and project variables),
/// which are not known here.
///
/// # Errors
/// A variable without its closing `}`.
pub fn expand_field<'a>(
    text: &str,
    fields: impl IntoIterator<Item = (&'a str, &'a str)> + Clone,
) -> Result<String, Error> {
    expand(text, |name| {
        let field = fields
            .clone()
            .into_iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.to_string());
        Ok(field.or_else(|| {
            SIM_VARIABLES
                .iter()
                .any(|sim| sim.eq_ignore_ascii_case(name))
                .then(String::new)
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_field() {
        let fields = [("Sim.Params", "dc=0 ac=1"), ("Description", "Source")];
        let expand = |text| expand_field(text, fields.iter().copied()).unwrap();
        assert_eq!(expand("${SIM.PARAMS}"), "dc=0 ac=1");
        assert_eq!(expand("${sim.params}"), "dc=0 ac=1");
        assert_eq!(expand("a${DESCRIPTION}b${SIM.TYPE}c"), "aSourcebc");
        assert_eq!(expand("${SIM.NAME}"), "");
        assert_eq!(
            expand("${REFERENCE} ${SIM.ENABLE}"),
            "${REFERENCE} ${SIM.ENABLE}"
        );
        assert_eq!(expand("no variables"), "no variables");
        assert!(expand_field("${SIM.PARAMS", fields.iter().copied()).is_err());
    }
}
