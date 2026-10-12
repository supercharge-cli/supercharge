//! Colored ASCII logo and its periodic diagonal glint.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::sync::OnceLock;

use crate::theme::Theme;

pub(super) const COLS: u16 = 63;
pub(super) const ROWS: u16 = 28;
const START: f64 = 0.5;
const PASS: f64 = 2.0;
const EVERY: f64 = 5.0;
const HALF: f64 = 5.0;
const LEAN: f64 = 0.9;
const SOLID: &[u8] = b"8dbqpPYOo0";

const PALETTE: [[u8; 3]; 48] = [
    [11, 13, 36],
    [249, 249, 251],
    [67, 145, 233],
    [112, 84, 230],
    [215, 215, 234],
    [63, 212, 248],
    [53, 55, 94],
    [141, 138, 165],
    [96, 98, 113],
    [251, 251, 252],
    [133, 184, 241],
    [162, 144, 239],
    [229, 229, 241],
    [130, 227, 250],
    [124, 125, 150],
    [181, 179, 197],
    [182, 182, 189],
    [253, 253, 254],
    [199, 222, 248],
    [212, 204, 248],
    [243, 243, 249],
    [197, 242, 253],
    [194, 195, 207],
    [221, 220, 228],
    [72, 82, 199],
    [249, 249, 251],
    [67, 145, 233],
    [112, 84, 230],
    [215, 215, 234],
    [63, 212, 248],
    [86, 89, 153],
    [141, 138, 165],
    [136, 143, 219],
    [251, 251, 252],
    [133, 184, 241],
    [162, 144, 239],
    [229, 229, 241],
    [130, 227, 250],
    [145, 147, 189],
    [181, 179, 197],
    [200, 203, 238],
    [253, 253, 254],
    [199, 222, 248],
    [212, 204, 248],
    [243, 243, 249],
    [197, 242, 253],
    [204, 205, 224],
    [221, 220, 228],
];

const MONO: &str = r#"

                       .qq__             .__.
                       '8888qq_     __p88PYYYP8qq_,
                        d8888888q__pP"'"""""^Y- '"Yq_,
                        '888888888P'          "^o_  "8q,
                         '888888"      _,.__q____     "8p,
                       _p8P888P'     ~P\p8888888888qq_  "8_
                 __  _pP'_pq88q_,     q8888888888888888q_,8p
                 ""_q8'q888888888,   p88888888888888888888p8|
     q888888888P  q8"'|8888888888b  q88888888888888888888p88|
         .__ppq_  8p)pq88888888888 q888888888888888888888888'
          """""'  _8q888888888888P 88888888888888888888888qP
       ._pppppq__q8""88888888888P  888888888888888888888888'
     _p88888888/YP"q8q_'"Y888P"'  ,"8888888888888888888888"
   ___""""8888888qpqpp88q__'"O8pq_)( "8888888888888888888P
  "YYY'   .q8888888888"""""8q_,"'"8q_,  ""88888888888888P
          '888888888P      '888q_ d888qq__  '""Y8888P\8"
        ___ '""88888q__    _888888P'"888888pqqq___pp8"'
        '"'  .  '"88888q)_p88Y8P"'____"YPP"_88Y8(_q888q_
            "Y"   8888888"'qp,_pp888P"^8q,_8888P"8888888,
                   "YPP"_p888888P\p8"_pq/8"O88b  q888888'
                  \p._p888888888b8P'q888\8' '"8q__888""
                   _p8888888PY""88__pq__8"      '""'
                   q888888P"  .8888888P"
                   "88P"'     '8888888"
            _,._qpqqqqpp-      '""""'
                ''""""""YYYYPPPPPPPPPYY^

"#;

const ART: &str = r#"

                       .qq__             .__.
                       '8888qq_     __p88888888qq_,
                        d8888888q__p888888888888888q_,
                        '88888888888888888888888888888q,
                         '888888888888888888888888888888p,
                       _p888888888888888888888888888888888_
                 __  _p888888888888888888888888888888888888p
                 ""_q888888888888888888888888888888888888888|
     q888888888P  q88888888888888888888888888888888888888888|
         .__ppq_  888888888888888888888888888888888888888888'
          """""'  _8888888888888888888888888888888888888888P
       ._ppqqqq__q88888888888888888888888888888888888888888'
     _p888888888888888888888888888888888888888888888888888"
   ___""""88888888888888888888888888888888888888888888888P
  "YYY'   .q88888888888888888888888888888888888888888888P
          '88888888888888888888888888888888888888888888"
        ___ '""88888888888888888888888888888888888888"'
        '"'  .  '"888888888888888888888888888888(_q888q_
            "Y"   8888888"'q8888888888888888888888888888,
                   "YPP"_p8888888888888888"O888888888888'
                  \p._p8888888888888888888' '"8888888""
                   _p8888888PY""888888888"      '""'
                   q888888P"  .8888888P"
                   "88P"'     '8888888"
            _,._qpqqqqpp-      '""""'
                ''""""""YYYYPPPPPPPPPYY^

"#;

const INK: &str = r#"

                       00000             0000
                       00666000     000000000000000
                        000066600000011111111111110000
                        00520000001111111111111111111000
                         002300011111111111111111111111000
                       000000004411111410030000000001111100
                 33  001444000041111111000000000000000001100
                 33000140000000301111006000000000000000300100
     55522222222  0041400055500500110000000050000000000034000
         3333333  0444400055500030110000000555000000000003000
          333333  000440005550060110000000555500000055500000
       00000400000044000000000004110000000222000000022000000
     000333344440144441110000044411100000003000000033300000
   3300000000334444444444111100044411100000000000003300000
  33333   00000000000011111004411104411111000000000000000
          0000000000111111110000440000001111111100060000
        522 0000000001111441066000014000000001111110000
        522  3  0000000044000000111114400144040000000000
            333   000000000004444444411111104001100000000
                   00000000000044100114440000004440000000
                  6600000000000440014444400 00044440000
                   00300300000000044444100      0000
                   030000000  0000000000
                   000000     000000000
            0000000000000      000000
                000000000000000000000000

"#;

fn lines(source: &str) -> impl Iterator<Item = &str> {
    source
        .strip_prefix('\n')
        .unwrap()
        .strip_suffix('\n')
        .unwrap()
        .split('\n')
}

fn palette_offset(theme: &Theme) -> Option<usize> {
    if theme.is_bandless() {
        return None;
    }
    match theme.bg_base {
        Color::Rgb(r, g, b) if (u16::from(r) * 3 + u16::from(g) * 6 + u16::from(b)) > 1275 => {
            Some(0)
        }
        _ => Some(24),
    }
}

pub(super) fn render(area: Rect, buf: &mut Buffer, theme: &Theme, elapsed: f64) {
    if area.width < COLS || area.height < ROWS {
        return;
    }
    let offset = palette_offset(theme);
    let pic = if offset.is_some() { ART } else { MONO };
    let x0 = area.x + (area.width - COLS) / 2;
    let y0 = area.y;
    static EXTENTS: OnceLock<(f64, f64)> = OnceLock::new();
    let (lo, span) = *EXTENTS.get_or_init(|| {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for grid in [MONO, ART] {
            for (y, line) in lines(grid).enumerate() {
                for (x, ch) in line.bytes().enumerate() {
                    if ch != b' ' {
                        let position = x as f64 + LEAN * y as f64;
                        lo = lo.min(position);
                        hi = hi.max(position);
                    }
                }
            }
        }
        (lo, hi - lo + 2.0 * HALF)
    });
    let at = if elapsed.is_finite() && elapsed >= START {
        lo - HALF + span * ((elapsed - START) % EVERY) / PASS
    } else {
        f64::NEG_INFINITY
    };

    for (y, (art, ink)) in lines(pic).zip(lines(INK)).enumerate() {
        debug_assert!(y < ROWS as usize && art.len() <= COLS as usize);
        for (x, &original) in art.as_bytes().iter().enumerate() {
            if original == b' ' {
                continue;
            }
            let distance = (x as f64 + LEAN * y as f64 - at).abs();
            let k = if distance < HALF {
                1.0 - distance / HALF
            } else {
                0.0
            };
            let k = k * k * (3.0 - 2.0 * k);
            let ch = if k > 0.55 && SOLID.contains(&original) {
                b'/'
            } else {
                original
            };
            let cell = &mut buf[(x0 + x as u16, y0 + y as u16)];
            cell.set_char(ch as char);
            if let Some(offset) = offset {
                let index = ink.as_bytes()[x] - b'0';
                debug_assert!(index < 8);
                let level = if k > 0.6 {
                    2
                } else if k > 0.25 {
                    1
                } else {
                    0
                };
                let [r, g, b] = PALETTE[offset + level * 8 + usize::from(index)];
                cell.fg = Color::Rgb(r, g, b);
            } else {
                cell.fg = theme.gray;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_grids_align_and_fit() {
        let mono: Vec<_> = lines(MONO).collect();
        let art: Vec<_> = lines(ART).collect();
        let ink: Vec<_> = lines(INK).collect();
        assert_eq!((mono.len(), art.len(), ink.len()), (28, 28, 28));
        for (picture, colors) in art.iter().zip(ink) {
            assert!(picture.is_ascii() && picture.len() <= 63);
            assert!(
                picture.bytes().enumerate().all(
                    |(x, c)| c == b' ' || matches!(colors.as_bytes().get(x), Some(b'0'..=b'7'))
                )
            );
        }
    }

    #[test]
    fn glint_changes_ink_then_rests() {
        let area = Rect::new(0, 0, COLS, ROWS);
        let theme = Theme::tokyonight();
        let mut initial = Buffer::empty(area);
        let mut moving = Buffer::empty(area);
        let mut resting = Buffer::empty(area);
        render(area, &mut initial, &theme, 0.0);
        render(area, &mut moving, &theme, 1.5);
        render(area, &mut resting, &theme, 3.0);
        assert_ne!(initial.content, moving.content);
        assert_eq!(initial.content, resting.content);
    }
}
