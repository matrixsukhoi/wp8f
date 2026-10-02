//! 环形滑动平均（由原 `sma.rs` 迁入）。**只搬家不改逻辑**：函数体、签名、可见性保持原样。

pub struct SimpleMovingAverage {
    data: Vec<f64>,
    n: usize,
    cnt: usize,
    sum: f64,
    avg: f64,
}

impl SimpleMovingAverage {
    pub fn new(n: usize) -> Self {
        Self {
            data: vec![0.0; n],
            n,
            cnt: 0,
            sum: 0.0,
            avg: 0.0,
        }
    }

    pub fn add(&mut self, value: f64) -> f64 {
        // `filled` 字段只被删掉的 _is_ready 读过，且恒等于 cnt >= n，去掉即可
        if self.cnt < self.n {
            self.data[self.cnt] = value;
            self.cnt += 1;
            self.sum += value;
            self.avg = self.sum / self.cnt as f64;
            self.avg
        } else {
            let idx = self.cnt % self.n;
            let old = self.data[idx];
            self.data[idx] = value;
            self.sum = self.sum - old + value;
            self.avg = self.sum / self.n as f64;
            self.cnt += 1;
            self.avg
        }
    }

    pub fn average(&self) -> f64 {
        self.avg
    }

    pub fn n(&self) -> usize {
        self.n
    }
}
