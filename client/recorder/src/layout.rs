#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

pub fn canvas(base: (u32, u32), max_height: u32) -> (u32, u32) {
    let (bw, bh) = (base.0.max(2), base.1.max(2));
    let height = (max_height.min(bh).max(2)) & !1;
    let width = ((bw as u64 * height as u64 / bh as u64) as u32).max(2) & !1;
    (width, height)
}

pub fn fit(source: (u32, u32), target: (u32, u32)) -> Rect {
    let (sw, sh) = (source.0.max(1) as u64, source.1.max(1) as u64);
    let (tw, th) = (target.0 as u64, target.1 as u64);
    let (width, height) = if sw * th > tw * sh {
        (tw, (sh * tw / sw).max(1))
    } else {
        ((sw * th / sh).max(1), th)
    };
    let (width, height) = (width.min(tw) as u32, height.min(th) as u32);
    Rect {
        x: (target.0 - width) / 2,
        y: (target.1 - height) / 2,
        width,
        height,
    }
}

pub fn client_box(
    frame_bounds: (i32, i32),
    client_origin: (i32, i32),
    client_size: (u32, u32),
    texture: (u32, u32),
) -> Option<Rect> {
    if client_size.0 == 0 || client_size.1 == 0 {
        return None;
    }
    let left = (client_origin.0 - frame_bounds.0).max(0) as u32;
    let top = (client_origin.1 - frame_bounds.1).max(0) as u32;
    if left >= texture.0 || top >= texture.1 {
        return None;
    }
    let width = (texture.0 - left).min(client_size.0).max(1);
    let height = (texture.1 - top).min(client_size.1).max(1);
    Some(Rect {
        x: left,
        y: top,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_keeps_the_window_shape_and_even_sizes() {
        assert_eq!(canvas((2560, 1440), 1080), (1920, 1080));
        assert_eq!(canvas((1280, 720), 1080), (1280, 720));
        assert_eq!(canvas((1281, 721), 1080), (1278, 720));
        assert_eq!(canvas((1600, 900), 720), (1280, 720));
        assert_eq!(canvas((800, 1200), 1080), (720, 1080));
        assert_eq!(canvas((0, 0), 1080), (2, 2));
    }

    #[test]
    fn fit_adds_bars_only_where_the_shape_differs() {
        assert_eq!(
            fit((1920, 1080), (1920, 1080)),
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(
            fit((1000, 1000), (1920, 1080)),
            Rect {
                x: 420,
                y: 0,
                width: 1080,
                height: 1080
            }
        );
        assert_eq!(
            fit((2000, 500), (1000, 1000)),
            Rect {
                x: 0,
                y: 375,
                width: 1000,
                height: 250
            }
        );
    }

    #[test]
    fn client_area_is_cut_out_of_the_window_frame() {
        let r = client_box((100, 50), (108, 81), (1280, 720), (1296, 809)).unwrap();
        assert_eq!(
            r,
            Rect {
                x: 8,
                y: 31,
                width: 1280,
                height: 720
            }
        );
        let full = client_box((0, 0), (0, 0), (1920, 1080), (1920, 1080)).unwrap();
        assert_eq!(
            full,
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
        let clipped = client_box((0, 0), (0, 0), (1920, 1080), (1600, 900)).unwrap();
        assert_eq!(
            clipped,
            Rect {
                x: 0,
                y: 0,
                width: 1600,
                height: 900
            }
        );
        assert_eq!(client_box((0, 0), (0, 0), (0, 0), (100, 100)), None);
    }
}
