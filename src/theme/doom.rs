//! Themes ported from doom-themes (https://github.com/doomemacs/themes, MIT).
//!
//! Each one keeps its theme's own palette and face mapping as written in its
//! `doom-<name>-theme.el`, with that theme's defaults for the options that
//! change them (no brighter comments, no brighter modeline, default region).
//! `doom-blend`, `doom-darken` and `doom-lighten` are ported too, so a face
//! the theme derives from its palette comes out the same colour here.

/// A 24-bit colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// `#rrggbb` as `0xrrggbb`.
const fn hex(rgb: u32) -> Rgb {
    Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// `(doom-blend a b alpha)`: `alpha` of `a`, the rest of `b`.
pub const fn blend(a: Rgb, b: Rgb, alpha: f32) -> Rgb {
    const fn mix(a: u8, b: u8, alpha: f32) -> u8 {
        (a as f32 * alpha + b as f32 * (1.0 - alpha) + 0.5) as u8
    }
    Rgb(mix(a.0, b.0, alpha), mix(a.1, b.1, alpha), mix(a.2, b.2, alpha))
}

/// `(doom-darken color alpha)`.
const fn darken(color: Rgb, alpha: f32) -> Rgb {
    blend(color, Rgb(0, 0, 0), 1.0 - alpha)
}

/// `(doom-lighten color alpha)`.
const fn lighten(color: Rgb, alpha: f32) -> Rgb {
    blend(color, Rgb(255, 255, 255), 1.0 - alpha)
}

/// The palette and faces of a doom theme that rrrev draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Doom {
    pub bg: Rgb,
    pub fg: Rgb,
    pub bg_alt: Rgb,

    pub red: Rgb,
    pub orange: Rgb,
    pub green: Rgb,
    pub yellow: Rgb,
    pub blue: Rgb,
    pub magenta: Rgb,
    pub cyan: Rgb,

    pub highlight: Rgb,
    pub region: Rgb,

    pub builtin: Rgb,
    pub comments: Rgb,
    pub doc_comments: Rgb,
    pub constants: Rgb,
    pub functions: Rgb,
    pub keywords: Rgb,
    pub methods: Rgb,
    pub operators: Rgb,
    pub types: Rgb,
    pub strings: Rgb,
    pub variables: Rgb,
    pub numbers: Rgb,

    pub vc_modified: Rgb,
    pub vc_added: Rgb,
    pub vc_deleted: Rgb,
}

pub const ONE: Doom = {
    let (bg, fg, bg_alt) = (hex(0x282c34), hex(0xbbc2cf), hex(0x21242b));
    let base5 = hex(0x5B6268);
    let (red, orange, green, yellow) = (hex(0xff6c6b), hex(0xda8548), hex(0x98be65), hex(0xECBE7B));
    let (blue, magenta, violet, cyan) = (hex(0x51afef), hex(0xc678dd), hex(0xa9a1e1), hex(0x46D9FF));
    Doom {
        bg, fg, bg_alt, red, orange, green, yellow, blue, magenta, cyan,
        highlight: blue,
        region: lighten(bg_alt, 0.15),
        builtin: magenta,
        comments: base5,
        doc_comments: lighten(base5, 0.25),
        constants: violet,
        functions: magenta,
        keywords: blue,
        methods: cyan,
        operators: blue,
        types: yellow,
        strings: green,
        variables: lighten(magenta, 0.4),
        numbers: orange,
        vc_modified: orange,
        vc_added: green,
        vc_deleted: red,
    }
};

pub const ONE_LIGHT: Doom = {
    let (bg, fg, bg_alt) = (hex(0xfafafa), hex(0x383a42), hex(0xf0f0f0));
    let base4 = hex(0x9ca0a4);
    let (red, orange, green, yellow) = (hex(0xe45649), hex(0xda8548), hex(0x50a14f), hex(0x986801));
    let (blue, magenta, violet, cyan) = (hex(0x4078f2), hex(0xa626a4), hex(0xb751b6), hex(0x0184bc));
    Doom {
        bg, fg, bg_alt, red, orange, green, yellow, blue, magenta, cyan,
        highlight: blue,
        region: darken(bg_alt, 0.1),
        builtin: magenta,
        comments: base4,
        doc_comments: darken(base4, 0.15),
        constants: violet,
        functions: magenta,
        keywords: red,
        methods: cyan,
        operators: blue,
        types: yellow,
        strings: green,
        variables: darken(magenta, 0.36),
        numbers: orange,
        vc_modified: orange,
        vc_added: green,
        vc_deleted: red,
    }
};

pub const NORD: Doom = {
    let (bg, fg, bg_alt) = (hex(0x2E3440), hex(0xECEFF4), hex(0x272C36));
    let (base4, base5, base7) = (hex(0x434C5E), hex(0x4C566A), hex(0xD8DEE9));
    let (red, orange, green, yellow) = (hex(0xBF616A), hex(0xD08770), hex(0xA3BE8C), hex(0xEBCB8B));
    let (blue, magenta, cyan, teal) = (hex(0x81A1C1), hex(0xB48EAD), hex(0x88C0D0), hex(0x8FBCBB));
    Doom {
        bg, fg, bg_alt, red, orange, green, yellow, blue, magenta, cyan,
        highlight: blue,
        region: base4,
        builtin: blue,
        comments: lighten(base5, 0.2),
        doc_comments: lighten(base5, 0.25),
        constants: blue,
        functions: cyan,
        keywords: blue,
        methods: cyan,
        operators: blue,
        types: teal,
        strings: green,
        variables: base7,
        numbers: magenta,
        vc_modified: orange,
        vc_added: green,
        vc_deleted: red,
    }
};

pub const NORD_LIGHT: Doom = {
    let (bg, fg, bg_alt) = (hex(0xE5E9F0), hex(0x3B4252), hex(0xD8DEE9));
    let (base4, base5) = (hex(0xB8C5DB), hex(0xAEBACF));
    let (red, orange, green, yellow) = (hex(0x99324B), hex(0xAC4426), hex(0x4F894C), hex(0x9A7500));
    let (blue, magenta, violet, cyan) = (hex(0x3B6EA8), hex(0x97365B), hex(0x842879), hex(0x398EAC));
    let teal = hex(0x29838D);
    Doom {
        bg, fg, bg_alt, red, orange, green, yellow, blue, magenta, cyan,
        highlight: blend(blue, bg, 0.8),
        region: base4,
        builtin: teal,
        comments: darken(base5, 0.2),
        doc_comments: darken(base5, 0.25),
        constants: magenta,
        functions: teal,
        keywords: blue,
        methods: teal,
        operators: blue,
        types: yellow,
        strings: green,
        variables: violet,
        numbers: magenta,
        vc_modified: orange,
        vc_added: green,
        vc_deleted: red,
    }
};

pub const GRUVBOX: Doom = {
    let (bg, fg, bg_alt, bg_alt2) = (hex(0x282828), hex(0xebdbb2), hex(0x1d2021), hex(0x504945));
    let (fg_alt, grey) = (hex(0xd5c4a1), hex(0x928374));
    let (red, orange, green, yellow) = (hex(0xfb4934), hex(0xfe8019), hex(0xb8bb26), hex(0xfabd2f));
    let (blue, magenta, violet, cyan) = (hex(0x83a598), hex(0xb16286), hex(0xd3869b), hex(0x8ec07c));
    Doom {
        bg, fg, bg_alt, red, orange, green, yellow, blue, magenta, cyan,
        highlight: yellow,
        region: bg_alt2,
        builtin: orange,
        comments: grey,
        doc_comments: lighten(fg_alt, 0.25),
        constants: violet,
        functions: green,
        keywords: red,
        methods: green,
        operators: fg,
        types: yellow,
        strings: green,
        variables: blue,
        numbers: violet,
        vc_modified: darken(cyan, 0.15),
        vc_added: darken(green, 0.15),
        vc_deleted: darken(red, 0.15),
    }
};

pub const GRUVBOX_LIGHT: Doom = {
    let (bg, fg, bg_alt) = (hex(0xfbf1c7), hex(0x282828), hex(0xf2e5bc));
    let base4 = hex(0xa89984);
    let (red, orange, green, yellow) = (hex(0x9d0006), hex(0xaf3a03), hex(0x79740e), hex(0xb57614));
    let (blue, magenta, violet, cyan) = (hex(0x076678), hex(0xb16286), hex(0x8f3f71), hex(0x427b58));
    Doom {
        bg, fg, bg_alt, red, orange, green, yellow, blue, magenta, cyan,
        highlight: base4,
        region: darken(bg_alt, 0.1),
        builtin: orange,
        comments: base4,
        doc_comments: green,
        constants: violet,
        functions: yellow,
        keywords: red,
        methods: cyan,
        operators: blue,
        types: violet,
        strings: green,
        variables: blue,
        numbers: violet,
        vc_modified: orange,
        vc_added: green,
        vc_deleted: red,
    }
};

pub const SOLARIZED_DARK: Doom = {
    let (bg, fg, bg_alt) = (hex(0x002b36), hex(0x839496), hex(0x00212B));
    let (base0, base5) = (hex(0x073642), hex(0x405A61));
    let (red, orange, green, yellow) = (hex(0xdc322f), hex(0xcb4b16), hex(0x859900), hex(0xb58900));
    let (blue, magenta, violet, cyan) = (hex(0x268bd2), hex(0xd33682), hex(0x6c71c4), hex(0x2aa198));
    let teal = hex(0x35a69c);
    Doom {
        bg, fg, bg_alt, red, orange, green, yellow, blue, magenta, cyan,
        highlight: blue,
        region: base0,
        builtin: blue,
        comments: base5,
        doc_comments: teal,
        constants: magenta,
        functions: blue,
        keywords: green,
        methods: cyan,
        operators: orange,
        types: yellow,
        strings: cyan,
        variables: violet,
        numbers: magenta,
        vc_modified: yellow,
        vc_added: green,
        vc_deleted: red,
    }
};

pub const SOLARIZED_LIGHT: Doom = {
    let (bg, fg, bg_alt) = (hex(0xFDF6E3), hex(0x556b72), hex(0xEEE8D5));
    let base6 = hex(0x96A7A9);
    let (red, orange, green, yellow) = (hex(0xdc322f), hex(0xcb4b16), hex(0x859900), hex(0xb58900));
    let (blue, magenta, violet, cyan) = (hex(0x268bd2), hex(0xd33682), hex(0x6c71c4), hex(0x2aa198));
    let teal = hex(0x35a69c);
    Doom {
        bg, fg, bg_alt, red, orange, green, yellow, blue, magenta, cyan,
        highlight: blue,
        region: darken(bg_alt, 0.1),
        builtin: magenta,
        comments: base6,
        doc_comments: teal,
        constants: violet,
        functions: magenta,
        keywords: green,
        methods: cyan,
        operators: blue,
        types: yellow,
        strings: cyan,
        variables: blue,
        numbers: violet,
        vc_modified: orange,
        vc_added: green,
        vc_deleted: red,
    }
};

pub const TOMORROW_NIGHT: Doom = {
    let (bg, fg, bg_alt) = (hex(0x1d1f21), hex(0xc5c8c6), hex(0x161719));
    let grey = hex(0x5a5b5a);
    let (red, orange, green, yellow) = (hex(0xcc6666), hex(0xde935f), hex(0xb5bd68), hex(0xf0c674));
    let (blue, magenta, violet, cyan) = (hex(0x81a2be), hex(0xc9b4cf), hex(0xb294bb), hex(0x8abeb7));
    Doom {
        bg, fg, bg_alt, red, orange, green, yellow, blue, magenta, cyan,
        highlight: blue,
        region: lighten(bg, 0.1),
        builtin: blue,
        comments: grey,
        doc_comments: lighten(grey, 0.14),
        constants: orange,
        functions: blue,
        keywords: violet,
        methods: blue,
        operators: fg,
        types: yellow,
        strings: green,
        variables: red,
        numbers: orange,
        vc_modified: darken(fg, 0.4),
        vc_added: green,
        vc_deleted: red,
    }
};

pub const TOMORROW_DAY: Doom = {
    let (bg, fg, bg_alt) = (hex(0xffffff), hex(0x4d4d4c), hex(0xf2f2f2));
    let (base1, grey) = (hex(0xe4e4e4), hex(0x8e908c));
    let (red, orange, green, yellow) = (hex(0xc82829), hex(0xf5871f), hex(0x718c00), hex(0xeab700));
    let (blue, magenta, violet, cyan) = (hex(0x4271ae), hex(0xc678dd), hex(0x8959a8), hex(0x8abeb7));
    Doom {
        bg, fg, bg_alt, red, orange, green, yellow, blue, magenta, cyan,
        highlight: blue,
        region: base1,
        builtin: blue,
        comments: grey,
        doc_comments: grey,
        constants: orange,
        functions: blue,
        keywords: violet,
        methods: blue,
        operators: fg,
        types: darken(yellow, 0.2),
        strings: green,
        variables: red,
        numbers: orange,
        vc_modified: lighten(yellow, 0.4),
        vc_added: lighten(green, 0.4),
        vc_deleted: red,
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blends_like_doom() {
        // doom-one's region: (doom-lighten "#21242b" 0.15).
        assert_eq!(lighten(hex(0x21242b), 0.15), Rgb(0x42, 0x45, 0x4b));
        assert_eq!(darken(hex(0xffffff), 0.5), Rgb(128, 128, 128));
        assert_eq!(blend(hex(0xff0000), hex(0x0000ff), 1.0), Rgb(255, 0, 0));
    }
}
