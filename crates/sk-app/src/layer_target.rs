//! What the sliders of the develop tabs change: the background or the active layer (the
//! tools work on the layer chosen in the layer list).

use sk_core::params::{Adjustments, LayerAdjustments};

/// The active layer's adjustments, as a copy that the tab edits and [`Target::store`] writes
/// back (so that the background stays borrowable next to it).
pub struct Target {
    layer: Option<(usize, LayerAdjustments)>,
}

impl Target {
    pub fn new(adj: &Adjustments, active: Option<usize>) -> Self {
        Self { layer: active.and_then(|i| adj.layers.get(i).map(|l| (i, l.adjustments))) }
    }

    pub fn is_layer(&self) -> bool {
        self.layer.is_some()
    }

    pub fn layer_mut(&mut self) -> Option<&mut LayerAdjustments> {
        self.layer.as_mut().map(|(_, l)| l)
    }

    /// The value a slider changes: the layer's field (a delta) or the background's.
    pub fn pick<'a>(&'a mut self, background: &'a mut f32, field: impl FnOnce(&'a mut LayerAdjustments) -> &'a mut f32) -> &'a mut f32 {
        match &mut self.layer {
            Some((_, l)) => field(l),
            None => background,
        }
    }

    /// Writes the layer's adjustments back.
    pub fn store(self, adj: &mut Adjustments) {
        if let Some((i, l)) = self.layer
            && let Some(layer) = adj.layers.get_mut(i)
        {
            layer.adjustments = l;
        }
    }
}
