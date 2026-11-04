const FILES: [(&str, &str); 4] = [
    ("en", include_str!("../lang/en.lang")),
    ("fr", include_str!("../lang/fr.lang")),
    ("es", include_str!("../lang/es.lang")),
    ("tr", include_str!("../lang/tr.lang")),
];
pub fn text(locale: &str, key: &str, args: &[&str]) -> String {
    let code = locale.get(..2).unwrap_or("en").to_ascii_lowercase();
    let lookup = |file: &str| {
        file.lines()
            .filter_map(|l| l.split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.to_owned())
    };
    let raw = FILES
        .iter()
        .find(|(c, _)| *c == code)
        .and_then(|(_, f)| lookup(f))
        .or_else(|| lookup(FILES[0].1))
        .unwrap_or_else(|| key.into());
    // Substitute once: user-supplied values cannot inject more placeholders.
    let mut out = String::new();
    let mut rest = raw.as_str();
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let Some(end) = tail.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        if let Some(arg) = tail[..end].parse::<usize>().ok().and_then(|i| args.get(i)) {
            out.push_str(arg);
        } else {
            out.push_str(&rest[start..start + end + 2]);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    #[test]
    fn all_keys_and_exact_placeholder_sets_match_in_four_languages() {
        let parse = |f: &str| -> BTreeMap<String, String> {
            let mut out = BTreeMap::new();
            for line in f.lines() {
                let (k, v) = line.split_once('=').unwrap();
                assert!(out.insert(k.into(), v.into()).is_none());
            }
            out
        };
        let slots = |v: &str| -> BTreeSet<String> {
            v.split('{')
                .skip(1)
                .filter_map(|p| p.split_once('}'))
                .map(|(k, _)| k.into())
                .collect()
        };
        let en = parse(FILES[0].1);
        for (_, f) in FILES {
            let got = parse(f);
            assert_eq!(
                en.keys().collect::<Vec<_>>(),
                got.keys().collect::<Vec<_>>()
            );
            for (key, v) in &en {
                assert_eq!(slots(v), slots(&got[key]), "{key}");
            }
        }
    }
    #[test]
    fn unknown_locale_falls_back_and_substitution_is_not_recursive() {
        assert_eq!(
            text("de_de", "corner", &["{1}", "x"]),
            "Corner {1} set at x."
        );
        assert_ne!(text("tr_tr", "denied", &[]), text("en", "denied", &[]));
    }
}
