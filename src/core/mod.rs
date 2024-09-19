pub mod stdout_reader;

pub fn round_f32(number: f32, precision: i32) -> f32 {
    let power = 10_f32.powi(precision);
    (number * power).round() / power
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round() {
        assert_eq!(round_f32(1.2345, 3).to_string(), "1.235")
    }
}
