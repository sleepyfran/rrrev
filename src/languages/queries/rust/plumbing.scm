(use_declaration) @plumbing
(extern_crate_declaration) @plumbing
(mod_item !body) @plumbing
; An attribute on an import, like `#[cfg(test)]`, goes with it.
((attribute_item) @plumbing . [(use_declaration) (extern_crate_declaration) (mod_item !body)])
