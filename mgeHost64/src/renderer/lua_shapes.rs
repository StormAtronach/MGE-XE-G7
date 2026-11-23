//! The line source `lua` of the debug views (W323 part 2a): the shapes that a mod sends with
//! `nirender.debug.line`, `box`, `sphere` and `clear`, one extension record each
//! ([`nirender_wgpu::debug_shapes_format`]). The plugin counts the segments of each frame's
//! calls and drops a shape past the limit before the stream ring; this list keeps the same
//! limit as a second guard, over all the shapes that it keeps.
//!
//! A shape with seconds stays until the game time passes its end: the time of the game's
//! simulation, which stops in a menu and in a pause, and in a replay the time of the capture.
//! A shape with no seconds is drawn in the frame of its record only. The shapes are kept
//! while the source is off too, so that a shape with seconds shows when the source goes on.

use nirender_wgpu::debug_lines::{LineGroup, LineList};
use nirender_wgpu::debug_shapes_format::{Kind, MOST_SEGMENTS, Shape};

/// A shape and the game time at which it goes; [`None`] for the frame of its record only.
#[derive(Clone, Copy, Debug)]
struct Kept {
    shape: Shape,
    until: Option<f32>,
}

/// The Lua shapes that the extension keeps.
#[derive(Clone, Debug, Default)]
pub struct LuaShapes {
    kept: Vec<Kept>,
    /// The segments of the kept shapes.
    segments: u32,
    /// The shapes refused by the limit, since the start.
    dropped: u64,
    /// The first record that was not read (another version, or an unknown kind) is in the log.
    unread_logged: bool,
}

impl LuaShapes {
    /// Takes the bytes of a record at the game time `now`. A record of another version or of
    /// an unknown kind is not read; the first one is in the log.
    pub fn take_record(&mut self, bytes: &[u8], now: f32) {
        match Shape::decode(bytes) {
            Some(shape) => self.take(shape, now),
            None => {
                if !self.unread_logged {
                    self.unread_logged = true;
                    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                    println!(
                        "extension record debug shapes: version {:?}, kind {:?}: not read (version 1 only; the first such record)",
                        word(0),
                        word(4)
                    );
                }
            }
        }
    }

    /// Takes a record at the game time `now`. A clear with a tag removes the shapes with its
    /// tag, a clear with the tag 0 every shape. A shape that would take the kept segments past
    /// [`MOST_SEGMENTS`] is refused and counted.
    pub fn take(&mut self, shape: Shape, now: f32) {
        if shape.kind == Kind::Clear {
            self.kept.retain(|kept| shape.tag != 0 && kept.shape.tag != shape.tag);
            self.count();
            return;
        }
        let segments = shape.kind.segments();
        if self.segments + segments > MOST_SEGMENTS {
            self.dropped += 1;
            return;
        }
        self.segments += segments;
        let until = (shape.seconds > 0.0).then_some(now + shape.seconds);
        self.kept.push(Kept { shape, until });
    }

    /// The end of a frame at the game time `now`: the shapes of that frame only go, and so do
    /// the shapes whose seconds have passed.
    pub fn end_frame(&mut self, now: f32) {
        self.kept.retain(|kept| kept.until.is_some_and(|until| now < until));
        self.count();
    }

    fn count(&mut self) {
        self.segments = self.kept.iter().map(|kept| kept.shape.kind.segments()).sum();
    }

    /// Adds the segments of the kept shapes.
    pub fn add_segments(&self, list: &mut LineList) {
        for kept in &self.kept {
            let shape = &kept.shape;
            let color = shape.color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            let color = [color[0], color[1], color[2], 255];
            let group = if shape.on_top { LineGroup::OnTop } else { LineGroup::Tested };
            let v = shape.values;
            let (a, b) = ([v[0], v[1], v[2]], [v[3], v[4], v[5]]);
            match shape.kind {
                Kind::Line => list.line(a, b, color, group),
                Kind::Box => list.aligned_box(a, b, color, group),
                Kind::Sphere => list.sphere(a, v[3], color, group),
                Kind::Clear => {}
            }
        }
    }

    /// The overlay line of the source: the shapes, their segments and the shapes refused.
    pub fn line(&self) -> String {
        format!(
            "debug lines lua: {} shapes, {} segments; {} refused past {MOST_SEGMENTS} segments",
            self.kept.len(),
            self.segments,
            self.dropped
        )
    }

    /// The shapes kept.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.kept.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nirender_wgpu::debug_lines::{BOX_SEGMENTS, SPHERE_SEGMENTS};
    use nirender_wgpu::debug_shapes_format::tag_hash;

    fn shape(kind: Kind, seconds: f32, tag: &str) -> Shape {
        Shape {
            kind,
            on_top: false,
            tag: tag_hash(tag),
            seconds,
            color: [1.0, 0.5, 0.0],
            values: [100.0, 200.0, 300.0, 110.0, 210.0, 310.0],
        }
    }

    /// A shape with no seconds stays for its frame; one with 2 seconds goes after 2 s of game
    /// time, and a pause (no time) keeps it.
    #[test]
    fn a_shape_stays_for_its_seconds() {
        let mut shapes = LuaShapes::default();
        shapes.take(shape(Kind::Line, 0.0, ""), 10.0);
        shapes.take(shape(Kind::Box, 2.0, ""), 10.0);
        assert_eq!(shapes.len(), 2);
        shapes.end_frame(10.0);
        assert_eq!(shapes.len(), 1);
        // A pause: the game time stays.
        for _ in 0..100 {
            shapes.end_frame(11.9);
        }
        assert_eq!(shapes.len(), 1);
        shapes.end_frame(12.0);
        assert_eq!(shapes.len(), 0);
    }

    /// clear(tag) removes only its tag; clear() removes every shape.
    #[test]
    fn a_clear_takes_its_tag() {
        let mut shapes = LuaShapes::default();
        shapes.take(shape(Kind::Sphere, 5.0, "a"), 0.0);
        shapes.take(shape(Kind::Sphere, 5.0, "b"), 0.0);
        shapes.take(shape(Kind::Line, 5.0, ""), 0.0);
        shapes.take(shape(Kind::Clear, 0.0, "a"), 0.0);
        assert_eq!(shapes.len(), 2);
        shapes.take(shape(Kind::Clear, 0.0, ""), 0.0);
        assert_eq!(shapes.len(), 0);
    }

    /// The segments of the shapes, in their colour and group; the sphere's radius is the
    /// fourth value.
    #[test]
    fn the_shapes_give_their_segments() {
        let mut shapes = LuaShapes::default();
        shapes.take(shape(Kind::Line, 0.0, ""), 0.0);
        shapes.take(shape(Kind::Box, 0.0, ""), 0.0);
        let mut on_top = shape(Kind::Sphere, 0.0, "");
        on_top.on_top = true;
        shapes.take(on_top, 0.0);
        let mut list = LineList::default();
        shapes.add_segments(&mut list);
        assert_eq!(list.len(), 1 + BOX_SEGMENTS + SPHERE_SEGMENTS);
        assert!(list.segments().iter().all(|s| s.color == [255, 128, 0, 255]));
        let sphere = &list.segments()[1 + BOX_SEGMENTS..];
        assert!(sphere.iter().all(|s| s.group == LineGroup::OnTop));
        let a = sphere[0].a;
        let radius = ((a[0] - 100.0).powi(2) + (a[1] - 200.0).powi(2) + (a[2] - 300.0).powi(2)).sqrt();
        assert!((radius - 110.0).abs() < 1e-9);
        assert!(
            list.segments()[..1 + BOX_SEGMENTS]
                .iter()
                .all(|s| s.group == LineGroup::Tested)
        );
    }

    /// The second guard: the kept segments stay at most the limit, by whole shapes.
    #[test]
    fn the_list_keeps_the_limit() {
        let mut shapes = LuaShapes::default();
        for _ in 0..70_000 / 12 {
            shapes.take(shape(Kind::Box, 0.0, ""), 0.0);
        }
        assert_eq!(shapes.len(), 5_461);
        assert_eq!(shapes.segments, 65_532);
        assert_eq!(shapes.dropped, 5_833 - 5_461);
        shapes.end_frame(0.0);
        assert_eq!(shapes.segments, 0);
    }
}
