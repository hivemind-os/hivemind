//! Embedding utilities shared across inference runtimes.

/// L2-normalize an embedding vector in place.
///
/// After normalization the vector has unit length (‖v‖₂ = 1), which makes
/// L2 (Euclidean) distance equivalent to cosine distance — a requirement
/// for models like BGE-small-en-v1.5 that are trained for cosine similarity.
///
/// A zero-magnitude vector is left unchanged (all zeros) to avoid division
/// by zero.
pub fn normalize_l2(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_vector_after_normalize() {
        let mut v = vec![3.0, 4.0];
        normalize_l2(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "norm should be 1.0, got {norm}");
    }

    #[test]
    fn zero_vector_unchanged() {
        let mut v = vec![0.0, 0.0, 0.0];
        normalize_l2(&mut v);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn already_normalized_stays_normalized() {
        let mut v = vec![1.0, 0.0, 0.0];
        normalize_l2(&mut v);
        assert!((v[0] - 1.0).abs() < 1e-6);
        assert!(v[1].abs() < 1e-6);
    }

    #[test]
    fn high_dimensional_vector_normalizes() {
        let mut v: Vec<f32> = (0..384).map(|i| i as f32).collect();
        normalize_l2(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm should be 1.0, got {norm}");
    }
}
