//! Terminal-aware and popular developer color themes.
//!
//! Every opaque palette maps the same semantic roles: canvas, popup surface,
//! borders, foreground, muted/subtle text, accent, success, warning, error, and
//! selection. Keeping those roles centralized prevents individual widgets from
//! drifting away from the selected theme.

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeKind {
    /// Inherit the terminal's foreground, background, and transparency.
    #[default]
    Terminal,
    /// bgrun's original opaque slate/indigo palette.
    Dark,
    /// Bright ANSI colors for maximum visibility.
    HighContrast,
    #[serde(alias = "catppuccin")]
    CatppuccinMocha,
    CatppuccinMacchiato,
    CatppuccinFrappe,
    CatppuccinLatte,
    Dracula,
    TokyoNight,
    Nord,
    GruvboxDark,
    GruvboxLight,
    SolarizedDark,
    SolarizedLight,
    RosePine,
    RosePineMoon,
    RosePineDawn,
    OneDark,
    GithubDark,
    GithubLight,
    Monokai,
    MaterialDark,
    AyuDark,
    AyuMirage,
    Kanagawa,
    Everforest,
    Oxocarbon,
    Poimandres,
    NightFox,
    Synthwave84,
}

impl ThemeKind {
    pub const ALL: [Self; 30] = [
        Self::Terminal,
        Self::Dark,
        Self::HighContrast,
        Self::CatppuccinMocha,
        Self::CatppuccinMacchiato,
        Self::CatppuccinFrappe,
        Self::CatppuccinLatte,
        Self::Dracula,
        Self::TokyoNight,
        Self::Nord,
        Self::GruvboxDark,
        Self::GruvboxLight,
        Self::SolarizedDark,
        Self::SolarizedLight,
        Self::RosePine,
        Self::RosePineMoon,
        Self::RosePineDawn,
        Self::OneDark,
        Self::GithubDark,
        Self::GithubLight,
        Self::Monokai,
        Self::MaterialDark,
        Self::AyuDark,
        Self::AyuMirage,
        Self::Kanagawa,
        Self::Everforest,
        Self::Oxocarbon,
        Self::Poimandres,
        Self::NightFox,
        Self::Synthwave84,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal (dynamic)",
            Self::Dark => "Dark",
            Self::HighContrast => "High contrast",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::CatppuccinMacchiato => "Catppuccin Macchiato",
            Self::CatppuccinFrappe => "Catppuccin Frappé",
            Self::CatppuccinLatte => "Catppuccin Latte",
            Self::Dracula => "Dracula",
            Self::TokyoNight => "Tokyo Night",
            Self::Nord => "Nord",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::GruvboxLight => "Gruvbox Light",
            Self::SolarizedDark => "Solarized Dark",
            Self::SolarizedLight => "Solarized Light",
            Self::RosePine => "Rosé Pine",
            Self::RosePineMoon => "Rosé Pine Moon",
            Self::RosePineDawn => "Rosé Pine Dawn",
            Self::OneDark => "One Dark",
            Self::GithubDark => "GitHub Dark",
            Self::GithubLight => "GitHub Light",
            Self::Monokai => "Monokai",
            Self::MaterialDark => "Material Dark",
            Self::AyuDark => "Ayu Dark",
            Self::AyuMirage => "Ayu Mirage",
            Self::Kanagawa => "Kanagawa",
            Self::Everforest => "Everforest",
            Self::Oxocarbon => "Oxocarbon",
            Self::Poimandres => "Poimandres",
            Self::NightFox => "Night Fox",
            Self::Synthwave84 => "Synthwave '84",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChromeStyle {
    #[default]
    Rounded,
    Minimal,
    Borderless,
}

impl ChromeStyle {
    pub const ALL: [Self; 3] = [Self::Rounded, Self::Minimal, Self::Borderless];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rounded => "Rounded cards",
            Self::Minimal => "Minimal",
            Self::Borderless => "Borderless",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub bg: Color,
    pub popup_bg: Color,
    pub border: Color,
    pub border_focus: Color,
    pub fg: Color,
    pub fg_muted: Color,
    pub fg_subtle: Color,
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub selection: Style,
}

#[derive(Debug, Clone, Copy)]
struct Palette {
    bg: Color,
    popup_bg: Color,
    border: Color,
    fg: Color,
    fg_muted: Color,
    fg_subtle: Color,
    accent: Color,
    ok: Color,
    warn: Color,
    err: Color,
}

impl Theme {
    pub fn new(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Terminal => Self {
                bg: Color::Reset,
                popup_bg: Color::Reset,
                border: Color::DarkGray,
                border_focus: Color::Cyan,
                fg: Color::Reset,
                fg_muted: Color::DarkGray,
                fg_subtle: Color::Gray,
                accent: Color::Cyan,
                ok: Color::Green,
                warn: Color::Yellow,
                err: Color::Red,
                selection: Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            },
            ThemeKind::Dark => Self::from_palette(
                Palette {
                    bg: rgb(0x000c_0d10),
                    popup_bg: rgb(0x0013_161d),
                    border: rgb(0x0030_3646),
                    fg: rgb(0x00e6_e8ec),
                    fg_muted: rgb(0x007a_8291),
                    fg_subtle: rgb(0x004b_5260),
                    accent: rgb(0x005f_d7ff),
                    ok: rgb(0x004a_de80),
                    warn: rgb(0x00fa_cc15),
                    err: rgb(0x00f8_7171),
                },
                rgb(0x0029_2e42),
                rgb(0x00e6_e8ec),
            ),
            ThemeKind::HighContrast => Self {
                bg: Color::Reset,
                popup_bg: Color::Reset,
                border: Color::White,
                border_focus: Color::LightCyan,
                fg: Color::Reset,
                fg_muted: Color::White,
                fg_subtle: Color::Gray,
                accent: Color::LightCyan,
                ok: Color::LightGreen,
                warn: Color::LightYellow,
                err: Color::LightRed,
                selection: Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            },
            ThemeKind::CatppuccinMocha => Self::from_palette(
                Palette {
                    bg: rgb(0x001e_1e2e),
                    popup_bg: rgb(0x0018_1825),
                    border: rgb(0x0045_475a),
                    fg: rgb(0x00cd_d6f4),
                    fg_muted: rgb(0x00a6_adc8),
                    fg_subtle: rgb(0x006c_7086),
                    accent: rgb(0x0089_b4fa),
                    ok: rgb(0x00a6_e3a1),
                    warn: rgb(0x00f9_e2af),
                    err: rgb(0x00f3_8ba8),
                },
                rgb(0x0031_3244),
                rgb(0x00cd_d6f4),
            ),
            ThemeKind::CatppuccinMacchiato => Self::from_palette(
                Palette {
                    bg: rgb(0x0024_273a),
                    popup_bg: rgb(0x001e_2030),
                    border: rgb(0x0049_4d64),
                    fg: rgb(0x00ca_d3f5),
                    fg_muted: rgb(0x0093_9ab7),
                    fg_subtle: rgb(0x006e_738d),
                    accent: rgb(0x00c6_a0f6),
                    ok: rgb(0x00a6_da95),
                    warn: rgb(0x00ee_d49f),
                    err: rgb(0x00ed_8796),
                },
                rgb(0x0036_3a4f),
                rgb(0x00ca_d3f5),
            ),
            ThemeKind::CatppuccinFrappe => Self::from_palette(
                Palette {
                    bg: rgb(0x0030_3446),
                    popup_bg: rgb(0x0029_2c3c),
                    border: rgb(0x0051_576d),
                    fg: rgb(0x00c6_d0f5),
                    fg_muted: rgb(0x00a5_adce),
                    fg_subtle: rgb(0x0073_7994),
                    accent: rgb(0x00ca_9ee6),
                    ok: rgb(0x00a6_d189),
                    warn: rgb(0x00e5_c890),
                    err: rgb(0x00e7_8284),
                },
                rgb(0x0041_4559),
                rgb(0x00c6_d0f5),
            ),
            ThemeKind::CatppuccinLatte => Self::from_palette(
                Palette {
                    bg: rgb(0x00ef_f1f5),
                    popup_bg: rgb(0x00e6_e9ef),
                    border: rgb(0x00ac_b0be),
                    fg: rgb(0x004c_4f69),
                    fg_muted: rgb(0x007c_7f93),
                    fg_subtle: rgb(0x009c_a0b0),
                    accent: rgb(0x0088_39ef),
                    ok: rgb(0x002f_8b1f),
                    warn: rgb(0x00a3_5c00),
                    err: rgb(0x00d2_0f39),
                },
                rgb(0x00cc_d0da),
                rgb(0x004c_4f69),
            ),
            ThemeKind::Dracula => Self::from_palette(
                Palette {
                    bg: rgb(0x0028_2a36),
                    popup_bg: rgb(0x0021_222c),
                    border: rgb(0x0044_475a),
                    fg: rgb(0x00f8_f8f2),
                    fg_muted: rgb(0x008b_e9fd),
                    fg_subtle: rgb(0x0062_72a4),
                    accent: rgb(0x00bd_93f9),
                    ok: rgb(0x0050_fa7b),
                    warn: rgb(0x00f1_fa8c),
                    err: rgb(0x00ff_5555),
                },
                rgb(0x0044_475a),
                rgb(0x00f8_f8f2),
            ),
            ThemeKind::TokyoNight => Self::from_palette(
                Palette {
                    bg: rgb(0x001a_1b26),
                    popup_bg: rgb(0x0016_161e),
                    border: rgb(0x0029_2e42),
                    fg: rgb(0x00c0_caf5),
                    fg_muted: rgb(0x007a_a2f7),
                    fg_subtle: rgb(0x0056_5f89),
                    accent: rgb(0x007a_a2f7),
                    ok: rgb(0x009e_ce6a),
                    warn: rgb(0x00e0_af68),
                    err: rgb(0x00f7_768e),
                },
                rgb(0x0029_2e42),
                rgb(0x00c0_caf5),
            ),
            ThemeKind::Nord => Self::from_palette(
                Palette {
                    bg: rgb(0x002e_3440),
                    popup_bg: rgb(0x0027_2c36),
                    border: rgb(0x003b_4252),
                    fg: rgb(0x00ec_eff4),
                    fg_muted: rgb(0x009a_acc5),
                    fg_subtle: rgb(0x007b_88a1),
                    accent: rgb(0x0088_c0d0),
                    ok: rgb(0x00a3_be8c),
                    warn: rgb(0x00eb_cb8b),
                    err: rgb(0x00bf_616a),
                },
                rgb(0x003b_4252),
                rgb(0x00ec_eff4),
            ),
            ThemeKind::GruvboxDark => Self::from_palette(
                Palette {
                    bg: rgb(0x0028_2828),
                    popup_bg: rgb(0x001d_2021),
                    border: rgb(0x0050_4945),
                    fg: rgb(0x00eb_dbb2),
                    fg_muted: rgb(0x00d5_c4a1),
                    fg_subtle: rgb(0x00a8_9984),
                    accent: rgb(0x0083_a598),
                    ok: rgb(0x00b8_bb26),
                    warn: rgb(0x00fa_bd2f),
                    err: rgb(0x00fb_4934),
                },
                rgb(0x0050_4945),
                rgb(0x00eb_dbb2),
            ),
            ThemeKind::GruvboxLight => Self::from_palette(
                Palette {
                    bg: rgb(0x00fb_f1c7),
                    popup_bg: rgb(0x00f2_e5bc),
                    border: rgb(0x00d5_c4a1),
                    fg: rgb(0x003c_3836),
                    fg_muted: rgb(0x0066_5c54),
                    fg_subtle: rgb(0x007c_6f64),
                    accent: rgb(0x0007_6678),
                    ok: rgb(0x0079_740e),
                    warn: rgb(0x00b5_7614),
                    err: rgb(0x009d_0006),
                },
                rgb(0x00d5_c4a1),
                rgb(0x003c_3836),
            ),
            ThemeKind::SolarizedDark => Self::from_palette(
                Palette {
                    bg: rgb(0x0000_2b36),
                    popup_bg: rgb(0x0007_3642),
                    border: rgb(0x0058_6e75),
                    fg: rgb(0x00ee_e8d5),
                    fg_muted: rgb(0x0093_a1a1),
                    fg_subtle: rgb(0x0065_7b83),
                    accent: rgb(0x0026_8bd2),
                    ok: rgb(0x0085_9900),
                    warn: rgb(0x00b5_8900),
                    err: rgb(0x00dc_322f),
                },
                rgb(0x0058_6e75),
                rgb(0x00fd_f6e3),
            ),
            ThemeKind::SolarizedLight => Self::from_palette(
                Palette {
                    bg: rgb(0x00fd_f6e3),
                    popup_bg: rgb(0x00ee_e8d5),
                    border: rgb(0x0093_a1a1),
                    fg: rgb(0x0040_555e),
                    fg_muted: rgb(0x006f_8388),
                    fg_subtle: rgb(0x0093_a1a1),
                    accent: rgb(0x0026_8bd2),
                    ok: rgb(0x0071_8300),
                    warn: rgb(0x009c_7400),
                    err: rgb(0x00dc_322f),
                },
                rgb(0x0007_3642),
                rgb(0x00fd_f6e3),
            ),
            ThemeKind::RosePine => Self::from_palette(
                Palette {
                    bg: rgb(0x0019_1724),
                    popup_bg: rgb(0x001f_1d2e),
                    border: rgb(0x0026_233a),
                    fg: rgb(0x00e0_def4),
                    fg_muted: rgb(0x0090_8caa),
                    fg_subtle: rgb(0x006e_6a86),
                    accent: rgb(0x00c4_a7e7),
                    ok: rgb(0x009c_cfd8),
                    warn: rgb(0x00f6_c177),
                    err: rgb(0x00eb_6f92),
                },
                rgb(0x0026_233a),
                rgb(0x00e0_def4),
            ),
            ThemeKind::RosePineMoon => Self::from_palette(
                Palette {
                    bg: rgb(0x0023_2136),
                    popup_bg: rgb(0x001f_1d2e),
                    border: rgb(0x003e_3a5c),
                    fg: rgb(0x00e0_def4),
                    fg_muted: rgb(0x0090_8caa),
                    fg_subtle: rgb(0x006e_6a86),
                    accent: rgb(0x00c4_a7e7),
                    ok: rgb(0x009c_cfd8),
                    warn: rgb(0x00f6_c177),
                    err: rgb(0x00eb_6f92),
                },
                rgb(0x003e_3a5c),
                rgb(0x00e0_def4),
            ),
            ThemeKind::RosePineDawn => Self::from_palette(
                Palette {
                    bg: rgb(0x00fa_f4ed),
                    popup_bg: rgb(0x00ff_faf3),
                    border: rgb(0x00df_dad9),
                    fg: rgb(0x0057_5279),
                    fg_muted: rgb(0x0079_7593),
                    fg_subtle: rgb(0x0098_93a5),
                    accent: rgb(0x0090_7aa9),
                    ok: rgb(0x0056_949f),
                    warn: rgb(0x00a3_5f00),
                    err: rgb(0x00b4_637a),
                },
                rgb(0x00df_dad9),
                rgb(0x0057_5279),
            ),
            ThemeKind::OneDark => Self::from_palette(
                Palette {
                    bg: rgb(0x0028_2c34),
                    popup_bg: rgb(0x0021_252b),
                    border: rgb(0x003e_4451),
                    fg: rgb(0x00ab_b2bf),
                    fg_muted: rgb(0x009d_a5b4),
                    fg_subtle: rgb(0x005c_6370),
                    accent: rgb(0x0061_afef),
                    ok: rgb(0x0098_c379),
                    warn: rgb(0x00e5_c07b),
                    err: rgb(0x00e0_6c75),
                },
                rgb(0x003e_4451),
                rgb(0x00ab_b2bf),
            ),
            ThemeKind::GithubDark => Self::from_palette(
                Palette {
                    bg: rgb(0x000d_1117),
                    popup_bg: rgb(0x0016_1b22),
                    border: rgb(0x0030_363d),
                    fg: rgb(0x00c9_d1d9),
                    fg_muted: rgb(0x009d_a7b3),
                    fg_subtle: rgb(0x006e_7681),
                    accent: rgb(0x0058_a6ff),
                    ok: rgb(0x003f_b950),
                    warn: rgb(0x00d2_9922),
                    err: rgb(0x00f8_5149),
                },
                rgb(0x0021_262d),
                rgb(0x00c9_d1d9),
            ),
            ThemeKind::GithubLight => Self::from_palette(
                Palette {
                    bg: rgb(0x00ff_ffff),
                    popup_bg: rgb(0x00f6_f8fa),
                    border: rgb(0x00d0_d7de),
                    fg: rgb(0x0024_292f),
                    fg_muted: rgb(0x0057_606a),
                    fg_subtle: rgb(0x008c_959f),
                    accent: rgb(0x0009_69da),
                    ok: rgb(0x001a_7f37),
                    warn: rgb(0x009a_6700),
                    err: rgb(0x00cf_222e),
                },
                rgb(0x00dd_f4ff),
                rgb(0x0024_292f),
            ),
            ThemeKind::Monokai => Self::from_palette(
                Palette {
                    bg: rgb(0x0027_2822),
                    popup_bg: rgb(0x001e_1f1c),
                    border: rgb(0x0049_483e),
                    fg: rgb(0x00f8_f8f2),
                    fg_muted: rgb(0x00c8_c8bd),
                    fg_subtle: rgb(0x0075_715e),
                    accent: rgb(0x0066_d9ef),
                    ok: rgb(0x00a6_e22e),
                    warn: rgb(0x00e6_db74),
                    err: rgb(0x00f9_2672),
                },
                rgb(0x0049_483e),
                rgb(0x00f8_f8f2),
            ),
            ThemeKind::MaterialDark => Self::from_palette(
                Palette {
                    bg: rgb(0x0026_3238),
                    popup_bg: rgb(0x001e_2a30),
                    border: rgb(0x0054_6e7a),
                    fg: rgb(0x00ee_ffff),
                    fg_muted: rgb(0x00b0_bec5),
                    fg_subtle: rgb(0x0078_909c),
                    accent: rgb(0x0082_aaff),
                    ok: rgb(0x00c3_e88d),
                    warn: rgb(0x00ff_cb6b),
                    err: rgb(0x00ff_5370),
                },
                rgb(0x0037_474f),
                rgb(0x00ee_ffff),
            ),
            ThemeKind::AyuDark => Self::from_palette(
                Palette {
                    bg: rgb(0x000a_0e14),
                    popup_bg: rgb(0x000d_1017),
                    border: rgb(0x001c_2230),
                    fg: rgb(0x00bf_bdb6),
                    fg_muted: rgb(0x008a_9199),
                    fg_subtle: rgb(0x006c_7689),
                    accent: rgb(0x00e6_b450),
                    ok: rgb(0x007f_d962),
                    warn: rgb(0x00ff_b454),
                    err: rgb(0x00d9_5763),
                },
                rgb(0x001c_2230),
                rgb(0x00bf_bdb6),
            ),
            ThemeKind::AyuMirage => Self::from_palette(
                Palette {
                    bg: rgb(0x001f_2430),
                    popup_bg: rgb(0x0019_1e2a),
                    border: rgb(0x0034_3a4a),
                    fg: rgb(0x00cc_cac2),
                    fg_muted: rgb(0x009a_9faa),
                    fg_subtle: rgb(0x0070_7a8c),
                    accent: rgb(0x00ff_cc66),
                    ok: rgb(0x0099_d794),
                    warn: rgb(0x00ff_d580),
                    err: rgb(0x00f2_8779),
                },
                rgb(0x0034_3a4a),
                rgb(0x00cc_cac2),
            ),
            ThemeKind::Kanagawa => Self::from_palette(
                Palette {
                    bg: rgb(0x001f_1f28),
                    popup_bg: rgb(0x0018_1820),
                    border: rgb(0x002a_2a37),
                    fg: rgb(0x00dc_d7ba),
                    fg_muted: rgb(0x00a3_a099),
                    fg_subtle: rgb(0x0072_7169),
                    accent: rgb(0x007e_9cd8),
                    ok: rgb(0x0098_bb6c),
                    warn: rgb(0x00e6_c384),
                    err: rgb(0x00e8_2424),
                },
                rgb(0x002a_2a37),
                rgb(0x00dc_d7ba),
            ),
            ThemeKind::Everforest => Self::from_palette(
                Palette {
                    bg: rgb(0x002d_353b),
                    popup_bg: rgb(0x0027_2e33),
                    border: rgb(0x0041_4b50),
                    fg: rgb(0x00d3_c6aa),
                    fg_muted: rgb(0x00a7_b8a0),
                    fg_subtle: rgb(0x0085_9289),
                    accent: rgb(0x00a7_c080),
                    ok: rgb(0x00a7_c080),
                    warn: rgb(0x00db_bc7f),
                    err: rgb(0x00e6_7e80),
                },
                rgb(0x0041_4b50),
                rgb(0x00d3_c6aa),
            ),
            ThemeKind::Oxocarbon => Self::from_palette(
                Palette {
                    bg: rgb(0x0016_1616),
                    popup_bg: rgb(0x0012_1212),
                    border: rgb(0x0039_3939),
                    fg: rgb(0x00f2_f4f8),
                    fg_muted: rgb(0x00c6_c6c6),
                    fg_subtle: rgb(0x008d_8d8d),
                    accent: rgb(0x0078_a9ff),
                    ok: rgb(0x0042_be65),
                    warn: rgb(0x00ff_eb3b),
                    err: rgb(0x00ff_7eb6),
                },
                rgb(0x0039_3939),
                rgb(0x00f2_f4f8),
            ),
            ThemeKind::Poimandres => Self::from_palette(
                Palette {
                    bg: rgb(0x001b_1e2b),
                    popup_bg: rgb(0x0014_1724),
                    border: rgb(0x0030_3347),
                    fg: rgb(0x00e4_f0fb),
                    fg_muted: rgb(0x00a6_accd),
                    fg_subtle: rgb(0x007b_87a8),
                    accent: rgb(0x0089_ddff),
                    ok: rgb(0x005d_e4c7),
                    warn: rgb(0x00ff_fac2),
                    err: rgb(0x00d0_679d),
                },
                rgb(0x0030_3347),
                rgb(0x00e4_f0fb),
            ),
            ThemeKind::NightFox => Self::from_palette(
                Palette {
                    bg: rgb(0x0019_2330),
                    popup_bg: rgb(0x0013_1a24),
                    border: rgb(0x0029_394f),
                    fg: rgb(0x00cd_cefb),
                    fg_muted: rgb(0x009c_a8ba),
                    fg_subtle: rgb(0x0071_839b),
                    accent: rgb(0x0072_b7b0),
                    ok: rgb(0x0081_b29a),
                    warn: rgb(0x00db_c074),
                    err: rgb(0x00c9_4f7c),
                },
                rgb(0x0029_394f),
                rgb(0x00cd_cefb),
            ),
            ThemeKind::Synthwave84 => Self::from_palette(
                Palette {
                    bg: rgb(0x0026_2335),
                    popup_bg: rgb(0x001e_1a2b),
                    border: rgb(0x0049_5495),
                    fg: rgb(0x00f9_7e72),
                    fg_muted: rgb(0x00b4_b8d8),
                    fg_subtle: rgb(0x0084_8bbd),
                    accent: rgb(0x00ff_7edb),
                    ok: rgb(0x0072_f1b8),
                    warn: rgb(0x00fe_de5d),
                    err: rgb(0x00fe_4450),
                },
                rgb(0x0034_294f),
                rgb(0x00f9_7e72),
            ),
        }
    }

    fn from_palette(palette: Palette, selection_bg: Color, selection_fg: Color) -> Self {
        Self {
            bg: palette.bg,
            popup_bg: palette.popup_bg,
            border: palette.border,
            border_focus: palette.accent,
            fg: palette.fg,
            fg_muted: palette.fg_muted,
            fg_subtle: palette.fg_subtle,
            accent: palette.accent,
            ok: palette.ok,
            warn: palette.warn,
            err: palette.err,
            selection: Style::default()
                .bg(selection_bg)
                .fg(selection_fg)
                .add_modifier(Modifier::BOLD),
        }
    }
}

const fn rgb(value: u32) -> Color {
    Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn terminal_theme_preserves_terminal_background() {
        let theme = Theme::new(ThemeKind::Terminal);
        assert_eq!(theme.bg, Color::Reset);
        assert_eq!(theme.popup_bg, Color::Reset);
        assert!(theme.selection.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn theme_labels_are_unique_and_complete() {
        let labels: HashSet<_> = ThemeKind::ALL.into_iter().map(ThemeKind::label).collect();
        assert_eq!(labels.len(), ThemeKind::ALL.len());
        assert!(ThemeKind::ALL.len() >= 30);
    }

    #[test]
    fn opaque_palettes_have_legible_semantic_colors() {
        let mut failures = Vec::new();
        for kind in ThemeKind::ALL {
            if matches!(kind, ThemeKind::Terminal | ThemeKind::HighContrast) {
                continue;
            }
            let theme = Theme::new(kind);
            let selection_fg = theme.selection.fg.expect("opaque selection foreground");
            let selection_bg = theme.selection.bg.expect("opaque selection background");
            let checks = [
                ("foreground", theme.fg, theme.bg, 4.5),
                ("popup foreground", theme.fg, theme.popup_bg, 4.5),
                ("muted foreground", theme.fg_muted, theme.bg, 3.0),
                ("accent", theme.accent, theme.bg, 3.0),
                ("success", theme.ok, theme.bg, 3.0),
                ("warning", theme.warn, theme.bg, 3.0),
                ("error", theme.err, theme.bg, 3.0),
                ("selection", selection_fg, selection_bg, 4.5),
            ];
            for (role, foreground, background, minimum) in checks {
                let ratio = contrast_ratio(foreground, background);
                if ratio < minimum {
                    failures.push(format!(
                        "{} {role} contrast {ratio:.2} is below {minimum:.2}",
                        kind.label(),
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    fn contrast_ratio(foreground: Color, background: Color) -> f64 {
        let foreground = relative_luminance(foreground);
        let background = relative_luminance(background);
        let lighter = foreground.max(background);
        let darker = foreground.min(background);
        (lighter + 0.05) / (darker + 0.05)
    }

    fn relative_luminance(color: Color) -> f64 {
        let Color::Rgb(red, green, blue) = color else {
            panic!("contrast checks require RGB colors, got {color:?}");
        };
        0.2126 * linear_channel(red)
            + 0.7152 * linear_channel(green)
            + 0.0722 * linear_channel(blue)
    }

    fn linear_channel(value: u8) -> f64 {
        let value = f64::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
}
