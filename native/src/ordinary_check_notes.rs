// Non-failing notes about declared names. Neither note resolves an identity or
// changes the record; they restore what the ordinary checker asks a person to read.
impl Projection<'_> {
    fn legend_notes(&self) -> Vec<String> {
        let declared = map(&self.base.reader.document)
            .ok()
            .and_then(|doc| doc.get("meta"))
            .and_then(|meta| map(meta).ok())
            .and_then(|meta| meta.get("prefixes"));
        let Some(declared) = declared.filter(|v| !matches!(v, V::Null)) else {
            return vec![];
        };
        let Ok(declared) = map(declared) else {
            return vec![
                "meta.prefixes is not a mapping of prefix to word - nothing is printed from it"
                    .into(),
            ];
        };
        let held = self
            .base
            .reader
            .ids
            .iter()
            .filter(|id| !F::BUILTINS.contains(&id.as_str()))
            .map(|id| id.split('.').next().unwrap())
            .collect::<BTreeSet<_>>();
        let mut notes = vec![];
        for (key, word) in declared {
            let key = declared.source_key(key).value();
            match key {
                V::Text(prefix) if !held.contains(prefix.as_str()) =>
                    notes.push(format!("meta.prefixes: {prefix} is held by nothing in the record")),
                V::Text(prefix) => {
                    if !matches!(word, V::Text(value) if value.chars().any(|c| !python_space(c))) {
                        notes.push(format!("meta.prefixes: {prefix} stands for {}, which is not a word - quote it", word.python_repr()));
                    }
                }
                _ => notes.push(format!("meta.prefixes: the key {} is not a word - yes, no, on, off, true and false are booleans to YAML unless quoted", key.python_repr())),
            }
        }
        notes
    }

    fn alias_notes(&self) -> Result<Vec<String>> {
        if string_is(&self.base.reader.fields["deps"], "also") {
            return Ok(vec![]);
        }
        static ID: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*$").unwrap()
        });
        let mut raws = vec![self.base.reader.raw.clone()];
        for (_, hypothesis) in &self.hypotheses {
            let hypothesis = map(hypothesis)?;
            if truth(get(hypothesis, "error")) {
                continue;
            }
            let mut raw = Map::new();
            for fields in F::collections(&hypothesis["document"])?.values() {
                for (id, body) in fields {
                    raw.insert(id.clone(), body.clone());
                }
            }
            raws.push(raw);
        }
        let mut live = raws[0]
            .keys()
            .filter(|id| !F::BUILTINS.contains(&id.as_str()))
            .collect::<BTreeSet<_>>();
        for raw in raws.iter().skip(1) {
            live.extend(raw.keys());
        }
        let mut seen = BTreeSet::new();
        let mut notes = vec![];
        for raw in &raws {
            let mut rows = raw.iter().collect::<Vec<_>>();
            rows.sort_by(|a, b| a.0.cmp(b.0));
            for (id, body) in rows {
                let Ok(body) = map(body) else {
                    continue;
                };
                let aliases = match body.get("also") {
                    Some(value @ V::Text(_)) => std::slice::from_ref(value),
                    Some(V::List(values)) => values.as_slice(),
                    _ => continue,
                };
                for alias in aliases {
                    let V::Text(alias) = alias else {
                        continue;
                    };
                    if ID.is_match(alias)
                        && alias != id
                        && live.contains(alias)
                        && seen.insert((id.clone(), alias.clone()))
                    {
                        notes.push(format!("{id} carries also: {alias}, and {alias} is an entry - a retirement that came back, or a sibling this record declares; the reader reads it as neither: same {id} {alias} folds them, distinct {id} {alias} \"why\" tells them apart"));
                    }
                }
            }
        }
        Ok(notes)
    }
}
