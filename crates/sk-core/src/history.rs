//! Names for edit steps (the history list): what changed between two sets of adjustments.
//! Compares fields directly (no serialization), so it is cheap enough to run every frame.

use crate::params::Adjustments;

/// "0,8", "-12", "1,25"
pub fn number(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.replace('.', ",")
}

/// What changed from `before` to `after`, e.g. "Belichtung 0,8" or "Kontrast, Lichter".
pub fn describe(before: &Adjustments, after: &Adjustments) -> String {
    let mut changes = changes(before, after);
    if changes.is_empty() && before != after {
        changes.push(("Bearbeitung", None));
    }
    match changes.as_slice() {
        [] => "Keine Änderung".into(),
        [(name, Some(v))] => format!("{name} {}", number(*v)),
        [(a, _)] => a.to_string(),
        [(a, _), (b, _)] => format!("{a}, {b}"),
        [(a, _), (b, _), rest @ ..] => format!("{a}, {b} +{}", rest.len()),
    }
}

/// The tools that differ between `before` and `after`, in panel order: the name and the new
/// value if the tool is a single number.
pub fn changes(before: &Adjustments, after: &Adjustments) -> Vec<(&'static str, Option<f32>)> {
    let (a, b) = (before, after);
    // (name, new value if it is a single number)
    let mut changes: Vec<(&'static str, Option<f32>)> = Vec::new();
    let mut push = |changed: bool, name: &'static str, value: Option<f32>| {
        if !changed {
            return;
        }
        match changes.iter_mut().find(|(n, _)| *n == name) {
            Some(c) => c.1 = None,
            None => changes.push((name, value)),
        }
    };
    for (name, x, y) in [
        ("Belichtung", a.exposure, b.exposure),
        ("Kontrast", a.contrast, b.contrast),
        ("Helligkeit", a.brightness, b.brightness),
        ("Sättigung", a.saturation, b.saturation),
        ("Dynamik", a.vibrance, b.vibrance),
        ("Lichter", a.highlights, b.highlights),
        ("Tiefen", a.shadows, b.shadows),
        ("Weiß", a.whites, b.whites),
        ("Schwarz", a.blacks, b.blacks),
    ] {
        push(x != y, name, Some(y));
    }
    push(a.white_balance != b.white_balance, "Weißabgleich", None);
    push(a.base_curve != b.base_curve, "Basiskurve", None);
    push(a.levels != b.levels, "Tonwerte", None);
    push(a.curves != b.curves, "Gradationskurve", None);
    push(a.rotation != b.rotation, "Drehen", None);
    let (g, h) = (&a.geometry, &b.geometry);
    push(g.crop != h.crop || g.aspect != h.aspect || g.aspect_portrait != h.aspect_portrait, "Zuschnitt", None);
    push(g.straighten != h.straighten, "Geraderichten", Some(h.straighten));
    push(g.flip_h != h.flip_h || g.flip_v != h.flip_v, "Spiegeln", None);
    push(g.keystone_v != h.keystone_v || g.keystone_h != h.keystone_h, "Keystone", None);
    push(a.lens != b.lens, "Objektivkorrektur", None);
    let (d, e) = (&a.detail, &b.detail);
    for (name, x, y) in [
        ("Schärfen", d.sharpen_amount, e.sharpen_amount),
        ("Schärfen", d.sharpen_radius, e.sharpen_radius),
        ("Schärfen", d.sharpen_threshold, e.sharpen_threshold),
        ("Rauschreduzierung", d.nr_luma, e.nr_luma),
        ("Rauschreduzierung", d.nr_color, e.nr_color),
        ("Klarheit", d.clarity, e.clarity),
        ("Struktur", d.structure, e.structure),
        ("Körnung", d.grain_amount, e.grain_amount),
        ("Körnung", d.grain_size, e.grain_size),
        ("Vignette", d.vignette_amount, e.vignette_amount),
        ("Dunst entfernen", d.dehaze, e.dehaze),
        ("Objektivunschärfe", a.lens_blur.amount, b.lens_blur.amount),
    ] {
        push(x != y, name, Some(y));
    }
    push(a.color_editor != b.color_editor, "Color Editor", None);
    push(a.skin_tone != b.skin_tone, "Hauttöne", None);
    push(a.color_balance != b.color_balance, "Color Balance", None);
    push(a.black_white != b.black_white, "Schwarzweiß", None);
    push(a.layers != b.layers, "Ebenen", None);
    push(a.retouch != b.retouch, "Retusche", None);
    push(a.removals != b.removals, "Entfernen", None);
    let (l, m) = (&a.lens_blur, &b.lens_blur);
    push(l.amount == m.amount && l != m, "Objektivunschärfe", None);
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_changes() {
        let a = Adjustments::default();
        let b = Adjustments { exposure: 0.8, ..a.clone() };
        assert_eq!(describe(&a, &b), "Belichtung 0,8");
        let mut c = b.clone();
        c.geometry.straighten = 2.0;
        c.geometry.keystone_v = 10.0;
        assert_eq!(describe(&b, &c), "Geraderichten, Keystone");
        let d = Adjustments { contrast: 10.0, highlights: 20.0, shadows: 5.0, ..c.clone() };
        assert_eq!(describe(&c, &d), "Kontrast, Lichter +1");
        let mut e = d.clone();
        e.detail.sharpen_amount = 50.0;
        assert_eq!(describe(&d, &e), "Schärfen 50");
        e.detail.sharpen_radius = 1.5;
        assert_eq!(describe(&d, &e), "Schärfen");
        assert_eq!(describe(&e, &e), "Keine Änderung");
    }
}
