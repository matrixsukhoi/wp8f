#[derive(Clone, Copy, Debug)]
pub struct TextScale {
    pub base_size: f32,
    pub scale: f32,
}

impl TextScale {
    pub fn new(base_size: f32) -> Self {
        Self {
            base_size,
            scale: 1.0,
        }
    }

    pub fn with_scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    pub fn data_size(&self) -> f32 {
        self.base_size * self.scale * 2.0
    }

    pub fn label_size(&self) -> f32 {
        self.base_size * self.scale
    }

    pub fn unit_size(&self) -> f32 {
        self.base_size * self.scale
    }

    pub fn row_height(&self) -> f32 {
        self.label_size() + self.unit_size()
    }


}

impl Default for TextScale {
    fn default() -> Self {
        Self::new(16.0)
    }
}
