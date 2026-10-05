pub fn turn_toward(current: f32, target: f32, limit: f32) -> f32 {
    let delta = (target - current + 180.0).rem_euclid(360.0) - 180.0;
    current + delta.clamp(-limit, limit)
}

pub fn intercept(position: [f64; 3], velocity: [f64; 3], ticks: f64) -> [f64; 3] {
    let t = ticks.clamp(0.0, 6.0);
    std::array::from_fn(|i| position[i] + velocity[i].clamp(-2.0, 2.0) * t)
}

/// Predict up to six ticks, moving before each turn; positive turns point +X toward +Z.
pub fn intercept_turning(
    mut position: [f64; 3],
    velocity: [f64; 3],
    ticks: f64,
    turn_rate: f64,
) -> [f64; 3] {
    if turn_rate == 0.0 {
        return intercept(position, velocity, ticks);
    }
    let mut velocity = cap_velocity(velocity);
    let t = ticks.clamp(0.0, 6.0);
    let turn = turn_rate
        .clamp(-std::f64::consts::FRAC_PI_6, std::f64::consts::FRAC_PI_6)
        .sin_cos();
    for _ in 0..t.floor() as u32 {
        position = std::array::from_fn(|i| position[i] + velocity[i]);
        velocity = rotate_horizontal(velocity, turn);
    }
    std::array::from_fn(|i| position[i] + velocity[i] * t.fract())
}

/// As `drop_intercept`, with bounded constant horizontal turn rate in radians/tick.
pub fn drop_intercept_turning(
    position: [f64; 3],
    own_velocity: [f64; 3],
    target: [f64; 3],
    target_velocity: [f64; 3],
    turn_rate: f64,
) -> Option<([f64; 3], f64, f64)> {
    if turn_rate == 0.0 {
        return drop_intercept(position, own_velocity, target, target_velocity);
    }
    drop_intercept_with_turn(position, own_velocity, target, target_velocity, turn_rate)
}

/// Predict the first downward crossing of the target's feet + 0.8 block center.
/// Returns (predicted target center, fractional ticks, horizontal miss in blocks).
/// Simulates up to 20 ticks after removing elytra, without collisions or steering.
pub fn drop_intercept(
    position: [f64; 3],
    own_velocity: [f64; 3],
    target: [f64; 3],
    target_velocity: [f64; 3],
) -> Option<([f64; 3], f64, f64)> {
    drop_intercept_with_turn(position, own_velocity, target, target_velocity, 0.0)
}

fn drop_intercept_with_turn(
    mut position: [f64; 3],
    mut own_velocity: [f64; 3],
    mut target: [f64; 3],
    mut target_velocity: [f64; 3],
    turn_rate: f64,
) -> Option<([f64; 3], f64, f64)> {
    if !turn_rate.is_finite()
        || ![position, own_velocity, target, target_velocity]
            .into_iter()
            .flatten()
            .all(f64::is_finite)
    {
        return None;
    }
    target_velocity = cap_velocity(target_velocity);
    let turn = turn_rate
        .clamp(-std::f64::consts::FRAC_PI_6, std::f64::consts::FRAC_PI_6)
        .sin_cos();
    target[1] += 0.8;
    for tick in 0..20 {
        let next_position = std::array::from_fn::<_, 3, _>(|i| position[i] + own_velocity[i]);
        let next_target = std::array::from_fn::<_, 3, _>(|i| target[i] + target_velocity[i]);
        let before = position[1] - target[1];
        let after = next_position[1] - next_target[1];
        if !next_position
            .into_iter()
            .chain(next_target)
            .chain([before, after])
            .all(f64::is_finite)
        {
            return None;
        }
        if own_velocity[1] < 0.0 && before >= 0.0 && after <= 0.0 {
            let fraction = if before == 0.0 {
                0.0
            } else {
                before / (before - after)
            };
            let point = std::array::from_fn(|i| target[i] + target_velocity[i] * fraction);
            let miss_x = position[0] + own_velocity[0] * fraction - point[0];
            let miss_z = position[2] + own_velocity[2] * fraction - point[2];
            let miss = miss_x.hypot(miss_z);
            return miss
                .is_finite()
                .then_some((point, f64::from(tick) + fraction, miss));
        }
        position = next_position;
        target = next_target;
        own_velocity[0] *= 0.91;
        own_velocity[1] = (own_velocity[1] - 0.08) * 0.98;
        own_velocity[2] *= 0.91;
        if turn_rate != 0.0 {
            target_velocity = rotate_horizontal(target_velocity, turn);
        }
    }
    None
}

fn cap_velocity(mut velocity: [f64; 3]) -> [f64; 3] {
    // Normalize before measuring length so even enormous teleport deltas cannot overflow.
    let largest = velocity.into_iter().map(f64::abs).fold(0.0, f64::max);
    if largest > 0.0 {
        let scaled = velocity.map(|value| value / largest);
        let length = scaled[0].hypot(scaled[1]).hypot(scaled[2]);
        if largest > 2.0 / length {
            velocity = scaled.map(|value| value * (2.0 / length));
        }
    }
    velocity
}

fn rotate_horizontal(velocity: [f64; 3], (sin, cos): (f64, f64)) -> [f64; 3] {
    [
        velocity[0] * cos - velocity[2] * sin,
        velocity[1],
        velocity[0] * sin + velocity[2] * cos,
    ]
}

pub fn smash_ready(vertical_velocity: f64, fallen: f64, distance_sq: f64) -> bool {
    vertical_velocity < -0.08 && fallen > 1.5 && distance_sq <= 9.0
}

pub fn passed_target(position: [f64; 3], target: [f64; 3], heading: [f64; 3]) -> bool {
    (target[0] - position[0]) * heading[0] + (target[2] - position[2]) * heading[2] < -1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_turn_rate_preserves_both_linear_interfaces() {
        for velocity in [[0.3, 0.2, -0.4], [100.0, 0.0, -100.0]] {
            for ticks in [-1.0, 0.0, 0.5, 3.5, 20.0] {
                let position = [10.0, 64.0, 20.0];
                assert_eq!(
                    intercept_turning(position, velocity, ticks, 0.0),
                    intercept(position, velocity, ticks)
                );
            }
            let position = [0.0, 70.0, 0.0];
            let own_velocity = [1.0, -0.8, 0.5];
            let target = [3.0, 64.0, 2.0];
            assert_eq!(
                drop_intercept_turning(position, own_velocity, target, velocity, 0.0),
                drop_intercept(position, own_velocity, target, velocity)
            );
        }
    }

    #[test]
    fn turning_lead_moves_before_rotating_and_interpolates_a_quarter_turn() {
        let rate = std::f64::consts::FRAC_PI_6;
        for sign in [-1.0, 1.0] {
            let point = intercept_turning([10.0, 64.0, 20.0], [1.0, 0.2, 0.0], 3.5, sign * rate);
            let expected = [
                11.5 + 3.0_f64.sqrt() / 2.0,
                64.7,
                20.0 + sign * (1.0 + 3.0_f64.sqrt() / 2.0),
            ];
            for (actual, expected) in point.into_iter().zip(expected) {
                assert!((actual - expected).abs() < 1e-10);
            }
            assert_eq!(
                point,
                intercept_turning([10.0, 64.0, 20.0], [1.0, 0.2, 0.0], 3.5, sign * 10.0)
            );
        }
    }

    #[test]
    fn turning_drop_finds_a_fractional_intercept_that_linear_prediction_misses() {
        let x = 3.0 + 3.0_f64.sqrt();
        for (ticks, height, z) in [
            (3.5, 68.55989168, 2.0 + 3.0_f64.sqrt()),
            (4.0, 69.14575136, x),
        ] {
            for speed in [2.0, f64::MAX] {
                let position = [x, height, z];
                let own_velocity = [0.0, -1.0, 0.0];
                let target = [0.0, 64.0, 0.0];
                let velocity = [speed, 0.0, 0.0];
                let (point, time, miss) = drop_intercept_turning(
                    position,
                    own_velocity,
                    target,
                    velocity,
                    std::f64::consts::FRAC_PI_6,
                )
                .unwrap();
                assert!((time - ticks).abs() < 1e-9);
                assert!((point[0] - x).abs() < 1e-9);
                assert!((point[2] - z).abs() < 1e-9);
                assert!(miss < 1e-9);
                let (_, _, linear_miss) =
                    drop_intercept(position, own_velocity, target, velocity).unwrap();
                assert!(linear_miss > 4.0);
            }
        }
    }

    #[test]
    fn turning_drop_rejects_nonfinite_turn_rate() {
        for rate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                drop_intercept_turning(
                    [0.0, 65.8, 0.0],
                    [0.0, -1.0, 0.0],
                    [0.0, 64.0, 0.0],
                    [0.0; 3],
                    rate,
                )
                .is_none()
            );
        }
    }

    #[test]
    fn turns_across_wrap_using_short_arc_and_rate_limit() {
        assert_eq!(turn_toward(175.0, -175.0, 6.0), 181.0);
        assert_eq!(turn_toward(-175.0, 175.0, 6.0), -181.0);
        assert_eq!(turn_toward(0.0, 90.0, 12.0), 12.0);
    }
    #[test]
    fn lead_tracks_moving_target_but_bounds_teleport_velocity() {
        assert_eq!(
            intercept([10.0, 64.0, 0.0], [0.3, 0.0, -0.2], 4.0),
            [11.2, 64.0, -0.8]
        );
        assert_eq!(
            intercept([0.0, 64.0, 0.0], [100.0, 0.0, 0.0], 20.0),
            [12.0, 64.0, 0.0]
        );
    }
    #[test]
    fn mace_strikes_only_while_falling_in_reach() {
        assert!(smash_ready(-0.4, 2.0, 8.5));
        assert!(!smash_ready(0.4, 2.0, 8.5));
        assert!(!smash_ready(-0.4, 1.0, 8.5));
        assert!(!smash_ready(-0.4, 2.0, 9.1));
    }
    #[test]
    fn pass_detects_crossing_plane_not_close_approach() {
        assert!(!passed_target(
            [0.0, 65.0, 0.0],
            [5.0, 65.0, 0.0],
            [1.0, 0.0, 0.0]
        ));
        assert!(passed_target(
            [7.0, 65.0, 0.0],
            [5.0, 65.0, 0.0],
            [1.0, 0.0, 0.0]
        ));
    }

    #[test]
    fn drop_leads_moving_target_and_measures_inherited_horizontal_momentum() {
        let (point, ticks, miss) = drop_intercept(
            [0.0, 65.8, 0.0],
            [2.0, -1.0, 0.0],
            [1.0, 64.0, 0.0],
            [1.0, 0.0, 0.0],
        )
        .unwrap();
        assert!((ticks - 1.0).abs() < 1e-10);
        assert!((point[0] - 2.0).abs() < 1e-10);
        assert!((point[1] - 64.8).abs() < 1e-10);
        assert!(miss < 1e-10);
        let (_, _, stopped_miss) = drop_intercept(
            [0.0, 65.8, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 64.0, 0.0],
            [1.0, 0.0, 0.0],
        )
        .unwrap();
        assert!((stopped_miss - 2.0).abs() < 1e-10);
    }

    #[test]
    fn drop_applies_air_drag_and_gravity_before_the_second_movement_tick() {
        // Two movement ticks: horizontal 2 + 1.82, downward 1 + 1.0584.
        let (point, ticks, miss) = drop_intercept(
            [0.0, 66.8584, 0.0],
            [2.0, -1.0, 0.0],
            [3.82, 64.0, 0.0],
            [0.0; 3],
        )
        .unwrap();
        assert!((ticks - 2.0).abs() < 1e-9);
        assert_eq!(point, [3.82, 64.8, 0.0]);
        assert!(miss < 1e-9);
    }

    #[test]
    fn drop_interpolates_the_crossing_instead_of_overshooting_by_a_full_tick() {
        let (point, ticks, miss) = drop_intercept(
            [0.0, 65.3, 0.0],
            [2.0, -1.0, 0.0],
            [1.0, 64.0, 0.0],
            [0.0; 3],
        )
        .unwrap();
        assert!((ticks - 0.5).abs() < 1e-10);
        assert_eq!(point, [1.0, 64.8, 0.0]);
        assert!(miss < 1e-10);
    }

    #[test]
    fn drop_rejects_no_window_and_an_ascending_bot_overtaken_by_the_target() {
        for (position, velocity, target_velocity) in [
            ([0.0, 63.0, 0.0], [0.0, -1.0, 0.0], [0.0; 3]),
            ([0.0, 200.0, 0.0], [0.0; 3], [0.0; 3]),
            ([0.0, 65.0, 0.0], [0.0, 0.5, 0.0], [0.0, 1.0, 0.0]),
        ] {
            assert!(
                drop_intercept(position, velocity, [0.0, 64.0, 0.0], target_velocity).is_none()
            );
        }
    }

    #[test]
    fn drop_bounds_teleport_speed_as_a_vector_and_preserves_its_direction() {
        for speed in [100.0, f64::MAX] {
            let (point, ticks, miss) = drop_intercept(
                [0.0, 65.8, 0.0],
                [0.0, -1.0, 0.0],
                [0.0, 64.0, 0.0],
                [speed, 0.0, -speed],
            )
            .unwrap();
            assert!((ticks - 1.0).abs() < 1e-10);
            assert!((point[0].hypot(point[2]) - 2.0).abs() < 1e-10);
            assert!((point[0] + point[2]).abs() < 1e-10);
            assert!((miss - 2.0).abs() < 1e-10);
        }
    }

    #[test]
    fn drop_rejects_nonfinite_inputs() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for input in 0..4 {
                let mut values = [
                    [0.0, 65.8, 0.0],
                    [0.0, -1.0, 0.0],
                    [0.0, 64.0, 0.0],
                    [0.0; 3],
                ];
                values[input][0] = invalid;
                assert!(drop_intercept(values[0], values[1], values[2], values[3]).is_none());
            }
        }
    }

    #[test]
    fn drop_tracks_a_rising_targets_future_center_height() {
        let (point, ticks, miss) = drop_intercept(
            [0.0, 65.8, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 64.0, 0.0],
            [0.0, 0.5, 0.0],
        )
        .unwrap();
        assert!((ticks - 2.0 / 3.0).abs() < 1e-10);
        assert!((point[1] - (64.8 + ticks * 0.5)).abs() < 1e-10);
        assert!(miss < 1e-10);
    }

    #[test]
    fn drop_can_predict_a_downward_window_after_the_bots_initial_ascent() {
        let (_, ticks, miss) = drop_intercept(
            [0.0, 66.8, 0.0],
            [0.0, 0.5, 0.0],
            [0.0, 64.0, 0.0],
            [0.0; 3],
        )
        .unwrap();
        assert!((10.0..20.0).contains(&ticks));
        assert!(miss < 1e-10);
    }
}
