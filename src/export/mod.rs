pub mod artwork;
pub mod kometa;
pub mod nfo;

/// What the tests of the documents written here say of them: that an XML
/// parser reads them to the end, and what it finds — not that the text looks
/// like XML to whoever wrote it.
#[cfg(test)]
pub(crate) mod xml {
    use quick_xml::{Reader, escape::resolve_predefined_entity, events::Event};

    /// One element read back: where it sits, its attributes, and the text
    /// directly in it, entities resolved.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Element {
        /// Its ancestors' names and its own, joined by `/`: `feed/entry/title`.
        pub path: String,
        pub attributes: Vec<(String, String)>,
        pub text: String,
    }

    impl Element {
        pub fn attribute(&self, name: &str) -> Option<&str> {
            self.attributes
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        }
    }

    /// Every element of a document, in the order they open.
    ///
    /// Panics, with the parser's own words, where the document is not
    /// well-formed: a tag left open or closed out of turn, an attribute
    /// unquoted or given twice, an entity XML does not have, a second root.
    pub fn read(document: &str) -> Vec<Element> {
        let mut reader = Reader::from_str(document);
        let mut elements: Vec<Element> = Vec::new();
        // The elements open now, as indexes into `elements`.
        let mut open: Vec<usize> = Vec::new();
        let mut roots = 0;

        let opened = |start: &quick_xml::events::BytesStart<'_>,
                      open: &[usize],
                      elements: &mut Vec<Element>|
         -> usize {
            let name = String::from_utf8(start.name().as_ref().to_vec()).expect("a name");
            let path = match open.last() {
                Some(&parent) => format!("{}/{name}", elements[parent].path),
                None => name,
            };
            let attributes = start
                .attributes()
                .map(|attribute| {
                    let attribute = attribute.expect("an attribute, quoted and given once");
                    (
                        String::from_utf8(attribute.key.as_ref().to_vec()).expect("a name"),
                        attribute
                            .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                            .expect("a value with the entities XML has")
                            .into_owned(),
                    )
                })
                .collect();
            elements.push(Element {
                path,
                attributes,
                text: String::new(),
            });
            elements.len() - 1
        };

        loop {
            match reader
                .read_event()
                .expect("a document an XML parser reads to its end")
            {
                Event::Start(start) => {
                    if open.is_empty() {
                        roots += 1;
                    }
                    let at = opened(&start, &open, &mut elements);
                    open.push(at);
                }
                Event::Empty(start) => {
                    if open.is_empty() {
                        roots += 1;
                    }
                    opened(&start, &open, &mut elements);
                }
                Event::End(_) => {
                    open.pop();
                }
                Event::Text(text) => {
                    if let Some(&at) = open.last() {
                        elements[at]
                            .text
                            .push_str(&text.decode().expect("text in UTF-8"));
                    }
                }
                Event::CData(data) => {
                    if let Some(&at) = open.last() {
                        elements[at]
                            .text
                            .push_str(&data.decode().expect("text in UTF-8"));
                    }
                }
                Event::GeneralRef(entity) => {
                    let name = entity.decode().expect("a name");
                    let resolved = match entity.resolve_char_ref().expect("a character") {
                        Some(c) => c.to_string(),
                        None => resolve_predefined_entity(&name)
                            .unwrap_or_else(|| panic!("&{name}; is not an entity XML has"))
                            .to_string(),
                    };
                    if let Some(&at) = open.last() {
                        elements[at].text.push_str(&resolved);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }

        assert!(open.is_empty(), "every element is closed: {open:?}");
        assert_eq!(roots, 1, "one root element");
        elements
    }

    /// The elements at a path, `feed/entry` or `movie/uniqueid`.
    pub fn at<'a>(elements: &'a [Element], path: &str) -> Vec<&'a Element> {
        elements.iter().filter(|e| e.path == path).collect()
    }

    /// The text of the elements at a path.
    pub fn texts<'a>(elements: &'a [Element], path: &str) -> Vec<&'a str> {
        at(elements, path)
            .into_iter()
            .map(|e| e.text.as_str())
            .collect()
    }
}
