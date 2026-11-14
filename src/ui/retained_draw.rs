use super::retained::*;
use crate::input::GamepadButton;
use crate::render::batch::{SpriteInstance, uv_region};
use crate::{
    camera::Rect,
    color::Color,
    ctx::Ctx,
    input::{Key, MouseButton},
    text::TextParams,
};
use glam::Vec2;

/// A click an input scenario asked for: where, and the text typed after it.
#[derive(Debug, Clone, Default)]
pub(crate) struct ScriptedClick {
    pub(crate) at: Vec2,
    pub(crate) text: String,
}

impl Ctx {
    pub(crate) fn update_retained_ui(&mut self) {
        let text = std::mem::take(&mut self.ui_state.retained_input_text);
        let mut input = self.retained_input(&self.ui, text);
        if let Some(click) = self.ui_state.scripted.take() {
            // Pressed and released in one frame: the UI clicks what is under
            // the pointer, as a quick tap of the mouse does.
            input.pointer = click.at;
            input.pressed = true;
            input.held = false;
            input.released = true;
            if !click.text.is_empty() {
                // Typed at the end of what the field holds.
                input.end = true;
                input.text.push_str(&click.text);
            }
        }
        self.ui.frame_with_text(self.window_size(), &input, &mut self.gfx.text);
        self.ui_state.retained_input_text = input.text;
        #[cfg(feature = "3d")]
        self.update_ui_panels();
    }

    /// This frame's input for `ui`, as the window's mouse, keyboard and
    /// gamepads give it; `text` (emptied) receives the typed text.
    pub(super) fn retained_input(&self, ui: &crate::ui::Ui, mut text: String) -> crate::ui::UiInput {
        text.clear();
        text.push_str(self.input.typed_text());
        let pressed = |key| self.input.key_pressed(key);
        let pad = |b| self.input.gamepads().iter().any(|&p| self.input.gamepad_pressed(p, b));
        let shift = self.input.key_down(Key::LeftShift) || self.input.key_down(Key::RightShift);
        let ctrl = self.input.key_down(Key::LeftCtrl)
            || self.input.key_down(Key::RightCtrl)
            || self.input.key_down(Key::LeftSuper)
            || self.input.key_down(Key::RightSuper);
        crate::ui::UiInput {
            pointer: self.input.mouse_pos(),
            pressed: self.input.mouse_pressed(MouseButton::Left),
            held: self.input.mouse_down(MouseButton::Left),
            released: self.input.mouse_released(MouseButton::Left),
            wheel: -self.input.mouse_wheel() * 40.0,
            next: pressed(Key::Tab) && !shift,
            previous: pressed(Key::Tab) && shift,
            direction: Vec2::new(
                (pressed(Key::Right) || pad(GamepadButton::DPadRight)) as u8 as f32
                    - (pressed(Key::Left) || pad(GamepadButton::DPadLeft)) as u8 as f32,
                (pressed(Key::Down) || pad(GamepadButton::DPadDown)) as u8 as f32
                    - (pressed(Key::Up) || pad(GamepadButton::DPadUp)) as u8 as f32,
            ),
            confirm: pressed(Key::Enter)
                || pad(GamepadButton::South)
                || (pressed(Key::Space)
                    && !ui
                        .focus()
                        .is_some_and(|id| ui.node(id).is_some_and(|n| n.widget == crate::ui::UiWidget::TextInput))),
            cancel: pressed(Key::Escape) || pad(GamepadButton::East),
            shift,
            select_all: ctrl && pressed(Key::A),
            backspace: pressed(Key::Backspace),
            delete: pressed(Key::Delete),
            home: pressed(Key::Home),
            end: pressed(Key::End),
            text,
        }
    }

    /// An input scenario's click on the node named `name` (and, with
    /// `text`, typing into it): queued for the retained UI's next frame, at
    /// the centre of the node as the UI lays it out now. Refused, with a
    /// sentence for the report, when no node has that name, it is not shown,
    /// something else is under its centre (a modal, a disabled node, an
    /// overlapping one), or `text` is for a node that is not a text input.
    pub(crate) fn script_ui_click(&mut self, name: &str, text: Option<&str>) -> Result<(), String> {
        let size = self.window_size();
        self.ui.layout(size);
        let ui = &self.ui;
        let id = ui.find(name).ok_or_else(|| format!("no UI node is named `{name}`"))?;
        if !ui.is_shown(id) {
            return Err(format!("UI node `{name}` is not on a shown screen"));
        }
        if text.is_some() && ui.node(id).is_some_and(|n| n.widget != UiWidget::TextInput) {
            return Err(format!("UI node `{name}` is not a text input"));
        }
        let rect = ui.rect(id).unwrap_or_default();
        let at = (rect.min + rect.max) / 2.0;
        let hit = ui.hit_test(at);
        let mut over = hit;
        while let Some(node) = over
            && node != id
        {
            over = ui.parent(node);
        }
        if over != Some(id) {
            let what = match hit {
                Some(other) => match ui.name(other) {
                    Some(other) => format!("`{other}`"),
                    None => "another node".to_string(),
                },
                None => "nothing that takes clicks (disabled, under a modal, or not on the top screen)".to_string(),
            };
            return Err(format!(
                "UI node `{name}` can't be clicked: at its centre ({:.0}, {:.0}) the pointer is over {what}",
                at.x, at.y
            ));
        }
        self.ui_state.scripted = Some(ScriptedClick {
            at,
            text: text.unwrap_or_default().to_string(),
        });
        Ok(())
    }

    pub(crate) fn draw_retained_ui(&mut self) {
        // Move the tree out while issuing draw calls; no callback borrowing the
        // game, context and node tree simultaneously is required.
        let spare = self.ui_state.retained_spare.take().unwrap_or_default();
        let mut ui = std::mem::replace(&mut self.ui, spare);
        self.draw_ui_tree(&mut ui, self.window_size());
        std::mem::swap(&mut self.ui, &mut ui);
        self.ui_state.retained_spare = Some(ui);
    }

    /// Draws `ui`, laid out in `viewport`, into `ui_list`.
    pub(crate) fn draw_ui_tree(&mut self, ui: &mut crate::ui::Ui, viewport: Vec2) {
        // Mutations made by game systems (including screen pushes) show now.
        ui.layout_with_text(viewport, &mut self.gfx.text);
        let scale = ui.scale();
        for (order, &id) in ui.visible.iter().enumerate() {
            let n = ui.node(id).unwrap();
            let z = 100.0 + order as f32 * 4.0;
            if ui.modal_root(id) {
                self.gfx.ui_list.set_clip(None);
                self.gfx.ui_fill(
                    Rect::new(0.0, 0.0, viewport.x, viewport.y),
                    Color::rgba(0.0, 0.0, 0.0, 0.55),
                    z - 1.0,
                );
            }
            self.gfx.ui_list.set_clip(Some(n.clip));
            let fill = if !n.enabled {
                n.paint.disabled
            } else if ui.pressed == Some(id) {
                n.paint.pressed
            } else if ui.hovered == Some(id) || ui.focus == Some(id) {
                n.paint.hover
            } else {
                n.paint.background
            };
            let width = if ui.focus == Some(id) {
                n.paint.border_width.max(2.0)
            } else {
                n.paint.border_width
            } * scale;
            if width > 0.0 {
                self.gfx.ui_fill(n.rect, n.paint.border, z);
            }
            let rect = n.rect.expand(-width);
            self.gfx.ui_fill(rect, fill, z + 0.1);
            if let Some(texture) = n.paint.texture {
                self.retained_image(
                    texture,
                    rect,
                    n.paint.slice * scale,
                    n.paint.slice,
                    n.paint.color,
                    z + 0.2,
                );
            }
            self.gfx.ui_list.set_clip(Some(intersect(n.clip, n.rect)));
            let layout = ui.tree.layout(n.layout).unwrap();
            let inset = Vec2::new(
                layout.padding.left + layout.border.left,
                layout.padding.top + layout.border.top,
            ) * scale;
            let content = Rect {
                min: n.rect.min + inset,
                max: n.rect.max
                    - Vec2::new(
                        layout.padding.right + layout.border.right,
                        layout.padding.bottom + layout.border.bottom,
                    ) * scale,
            };
            let mut text_rect = content;
            if n.widget == crate::ui::UiWidget::Toggle {
                let side = content.size().y.min(20.0 * scale);
                self.gfx.ui_fill(
                    Rect::new(content.min.x, content.min.y, side, side),
                    n.paint.border,
                    z + 0.4,
                );
                if n.value > 0.5 {
                    self.gfx.ui_fill(
                        Rect::new(
                            content.min.x + 4.0 * scale,
                            content.min.y + 4.0 * scale,
                            (side - 8.0 * scale).max(0.0),
                            (side - 8.0 * scale).max(0.0),
                        ),
                        n.paint.color,
                        z + 0.5,
                    );
                }
                text_rect.min.x += side + 6.0 * scale;
            }
            if n.widget == crate::ui::UiWidget::Slider {
                self.gfx.ui_fill(
                    Rect::new(
                        content.min.x,
                        content.center().y - 2.0 * scale,
                        content.size().x,
                        4.0 * scale,
                    ),
                    n.paint.border,
                    z + 0.4,
                );
                self.gfx.ui_fill(
                    Rect::new(
                        content.min.x + n.value * (content.size().x - 8.0 * scale),
                        content.min.y,
                        8.0 * scale,
                        content.size().y,
                    ),
                    n.paint.color,
                    z + 0.5,
                );
            }
            let font_size = n.paint.font_size * scale;
            let mut origin = text_rect.min;
            if n.widget == crate::ui::UiWidget::TextInput && ui.focus == Some(id) {
                let caret = self.gfx.caret_x(&n.text, n.paint.font, font_size, n.caret);
                origin.x -= (caret - text_rect.size().x + 3.0 * scale).max(0.0);
                // The selection, one span or several where it crosses a
                // change of direction.
                let mut spans = std::mem::take(&mut self.gfx.selection_scratch);
                self.gfx.selection_spans(
                    &n.text,
                    n.paint.font,
                    font_size,
                    n.anchor.min(n.caret),
                    n.anchor.max(n.caret),
                    &mut spans,
                );
                for &[a, b] in &spans {
                    if a != b {
                        self.gfx.ui_fill(
                            Rect::new(origin.x + a, origin.y, b - a, font_size),
                            n.paint.hover,
                            z + 0.6,
                        );
                    }
                }
                self.gfx.selection_scratch = spans;
                self.gfx.ui_fill(
                    Rect::new(origin.x + caret, origin.y, scale, font_size),
                    n.paint.color,
                    z + 0.8,
                );
            }
            let params = TextParams {
                font: n.paint.font,
                size: font_size,
                color: n.paint.color,
                z: z + 0.7,
            };
            // A text field keeps its text at its left edge, where its caret
            // and selection are measured from; labels and buttons align
            // right-to-left paragraphs from the right.
            let directional = n.widget != crate::ui::UiWidget::TextInput;
            self.retained_text(
                &n.text,
                origin,
                text_rect.size().x,
                params,
                (n.paint.wrap, directional),
                n.paint.align,
            );
            if n.widget == crate::ui::UiWidget::Scroll && n.max_scroll.y > 0.0 {
                let h = n.rect.size().y;
                let thumb = (h * h / (h + n.max_scroll.y * scale)).max(20.0 * scale).min(h);
                let y = n.rect.min.y + (h - thumb) * n.scroll.y / n.max_scroll.y;
                self.gfx.ui_fill(
                    Rect::new(n.rect.max.x - 8.0 * scale, y, 6.0 * scale, thumb),
                    n.paint.border,
                    z + 0.9,
                );
            }
        }
        self.gfx.ui_list.set_clip(None);
    }
    fn retained_text(
        &mut self,
        text: &str,
        origin: Vec2,
        width: f32,
        params: TextParams,
        (wrap, directional): (bool, bool),
        align: crate::ui::UiTextAlign,
    ) {
        let mut y = origin.y;
        for paragraph in text.split('\n') {
            let mut rest = paragraph;
            loop {
                let end = if wrap {
                    wrap_end(&mut self.gfx.text, rest, params.font, params.size, width)
                } else {
                    rest.len()
                };
                let line = &rest[..end];
                let w = self.gfx.measure_text_ex(line, params.font, params.size).x;
                // Start and End follow the paragraph's direction: a
                // right-to-left paragraph starts at the right.
                let rtl = directional && crate::text::Text::is_rtl(paragraph);
                let x = origin.x
                    + match (align, rtl) {
                        (crate::ui::UiTextAlign::Start, false) | (crate::ui::UiTextAlign::End, true) => 0.0,
                        (crate::ui::UiTextAlign::Center, _) => (width - w) * 0.5,
                        (crate::ui::UiTextAlign::End, false) | (crate::ui::UiTextAlign::Start, true) => width - w,
                    };
                self.gfx.draw_text_ex(line, Vec2::new(x, y), params);
                y += self.gfx.measure_text_ex(line, params.font, params.size).y;
                if end == rest.len() {
                    break;
                }
                rest = rest[end..].trim_start();
            }
        }
    }
    fn retained_image(
        &mut self,
        texture: crate::texture::Texture,
        rect: Rect,
        border: f32,
        source_border: f32,
        color: Color,
        z: f32,
    ) {
        let source = texture.size();
        let src = source_border.min(source.x * 0.5).min(source.y * 0.5).max(0.0);
        let bx = border.min(rect.size().x * 0.5).max(0.0);
        let by = border.min(rect.size().y * 0.5).max(0.0);
        let xs = [0.0, bx, rect.size().x - bx, rect.size().x];
        let ys = [0.0, by, rect.size().y - by, rect.size().y];
        let us = [0.0, src / source.x, 1.0 - src / source.x, 1.0];
        let vs = [0.0, src / source.y, 1.0 - src / source.y, 1.0];
        for y in 0..3 {
            for x in 0..3 {
                let size = Vec2::new(xs[x + 1] - xs[x], ys[y + 1] - ys[y]);
                if size.cmpgt(Vec2::ZERO).all() {
                    self.gfx.ui_list.push(
                        texture.id,
                        z,
                        SpriteInstance::new(
                            (rect.min + Vec2::new(xs[x], ys[y])).into(),
                            size.into(),
                            [0.0, 0.0],
                            0.0,
                            color.to_rgba8(),
                        )
                        .with_uv(uv_region([us[x], vs[y]], [us[x + 1] - us[x], vs[y + 1] - vs[y]])),
                    );
                }
            }
        }
    }
}
