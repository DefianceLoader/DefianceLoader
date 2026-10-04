//! Place the fixed ammunition tooltip beside the occupied, compacted grid.
//! All rectangles use the GUI's own coordinates, including its scale. Movement
//! translates the complete widget tree; no menu or widget pointer is retained.
use super::{read, Pair};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
}

impl Rect {
    fn from(values: [i32; 4]) -> Option<Self> {
        let [left, top, right, bottom] = values.map(i64::from);
        (left < right && top < bottom).then_some(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

/// Prefer the grid's right edge, then the space above it. The other sides
/// handle small viewports; retain the current position when no side can fit.
fn origin(grid: Rect, tooltip: Rect, viewport: Rect) -> Option<[i32; 2]> {
    const GAP: i64 = 2;
    let width = tooltip.right - tooltip.left;
    let height = tooltip.bottom - tooltip.top;
    let max_x = viewport.right - width;
    let max_y = viewport.bottom - height;
    if max_x < viewport.left || max_y < viewport.top {
        return None;
    }
    let x = grid.left.clamp(viewport.left, max_x);
    let y = (grid.bottom - height).clamp(viewport.top, max_y);
    let point = if grid.right + GAP <= max_x {
        [(grid.right + GAP).max(viewport.left), y]
    } else if grid.top - GAP - height >= viewport.top {
        [x, (grid.top - GAP - height).min(max_y)]
    } else if grid.left - GAP - width >= viewport.left {
        [(grid.left - GAP - width).min(max_x), y]
    } else if grid.bottom + GAP <= max_y {
        [x, (grid.bottom + GAP).max(viewport.top)]
    } else {
        return None;
    };
    Some([i32::try_from(point[0]).ok()?, i32::try_from(point[1]).ok()?])
}

unsafe fn rectangle(widget: usize) -> Option<Rect> {
    let get = std::mem::transmute::<usize, unsafe extern "C" fn(usize) -> *const [i32; 4]>(read(
        read(widget) + 0x40,
    ));
    let pointer = get(widget);
    pointer.as_ref().and_then(|value| Rect::from(*value))
}

pub(super) unsafe fn place(menu: usize, count: usize, field: usize, translate: Pair) {
    let owner = read(menu + 0x120);
    if owner == 0 {
        return;
    }
    let controller = read(owner + field);
    if controller == 0 || read(controller + 0x38) != owner {
        return;
    }
    let root = read(controller + 0x20);
    if root == 0 {
        return;
    }
    // The topmost live parent is the GUI viewport, rather than the ammo panel's
    // narrow anchor rectangle. Bound the walk so malformed trees cannot loop.
    let mut viewport = root;
    for _ in 0..64 {
        let parent = read(viewport + 0x38);
        if parent == 0 {
            break;
        }
        viewport = parent;
    }
    if viewport == root || read(viewport + 0x38) != 0 {
        return;
    }
    let (Some(tooltip), Some(viewport)) = (rectangle(root), rectangle(viewport)) else {
        return;
    };
    let mut grid: Option<Rect> = None;
    for index in 0..count {
        let widget = read(menu + 0x180 + index * 0xb8 + 0x18);
        if widget != 0 && *((widget + 0x5b) as *const u8) != 0 {
            if let Some(rect) = rectangle(widget) {
                grid = Some(grid.map_or(rect, |grid| grid.union(rect)));
            }
        }
    }
    let Some(point) = grid.and_then(|grid| origin(grid, tooltip, viewport)) else {
        return;
    };
    let (Ok(dx), Ok(dy)) = (
        i32::try_from(i64::from(point[0]) - tooltip.left),
        i32::try_from(i64::from(point[1]) - tooltip.top),
    ) else {
        return;
    };
    if dx != 0 || dy != 0 {
        let delta = [dx, dy];
        translate(root, delta.as_ptr() as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(values: [i32; 4]) -> Rect {
        Rect::from(values).unwrap()
    }

    #[test]
    fn placement_uses_visible_grid_and_viewport_coordinates() {
        let tip = rect([831, 848, 1232, 1060]);
        let screen = rect([0, 0, 1920, 1080]);
        assert_eq!(
            origin(rect([647, 852, 917, 1062]), tip, screen),
            Some([919, 850])
        );
        assert_eq!(
            origin(rect([647, 992, 735, 1062]), tip, screen),
            Some([737, 850])
        );
        assert_eq!(
            origin(rect([647, 852, 1757, 1062]), tip, screen),
            Some([647, 638])
        );
        // A shifted, scaled viewport must not be mistaken for a 1920px screen.
        assert_eq!(
            origin(
                rect([423, 526, 559, 632]),
                rect([0, 0, 201, 106]),
                rect([100, 100, 1060, 640])
            ),
            Some([561, 526])
        );
    }

    #[test]
    fn cramped_viewports_use_other_sides_or_keep_current_placement() {
        let tip = rect([0, 0, 100, 80]);
        let screen = rect([0, 0, 400, 300]);
        assert_eq!(
            origin(rect([250, 20, 400, 150]), tip, screen),
            Some([148, 70])
        );
        assert_eq!(origin(rect([0, 0, 400, 150]), tip, screen), Some([0, 152]));
        assert_eq!(origin(screen, tip, screen), None);
        assert_eq!(origin(screen, rect([0, 0, 500, 80]), screen), None);
        assert_eq!(Rect::from([4, 4, 4, 8]), None);
    }
}
