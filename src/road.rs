use std::{cell::RefCell, f64::consts::PI, rc::Rc, u32};

use itertools::Itertools;
use nalgebra::{vector, Point2, Point3};
use parry2d_f64::{
    math::Isometry,
    na::point,
    query::{self, ClosestPoints},
    shape::Shape,
};
use rand::prelude::StdRng;
use rvx::{Rvx, RvxColor};

use crate::{
    arg_parameters::Parameters,
    belief::Belief,
    car::SPEED_DEFAULT,
    cost::Cost,
    mpdm::{make_obstacle_vehicle_policy_choices, make_policy_choices},
    side_control::SideControlTrait,
    side_policies::SidePolicy,
};
use crate::{car::PRIUS_MAX_STEER, forward_control::ForwardControlTrait};

use crate::side_policies::SidePolicyTrait;

use crate::car::{Car, BREAKING_ACCEL};

pub const LANE_WIDTH: f64 = 3.7;
pub const ROAD_DASH_LENGTH: f64 = 3.0;
pub const ROAD_DASH_DIST: f64 = 9.0;
pub const ROAD_LENGTH: f64 = 500.0;

pub const SIDE_MARGIN: f64 = 0.0;

#[derive(Clone)]
pub struct Road {
    pub params: Rc<Parameters>,
    pub t: f64,           // current time in seconds
    pub timesteps: usize, // current time in timesteps (related by DT)
    pub cars: Vec<Car>,
    pub belief: Option<Rc<Belief>>,
    pub last_ego: Car,
    pub cost: Cost,
    pub ego_is_safe: bool,
    pub car_traces: Option<Vec<Vec<(Point3<f64>, u32)>>>,
    pub trajectory_buffer: Vec<Point2<f64>>,
    pub debug: bool,
}

fn range_dist(low_a: f64, high_a: f64, low_b: f64, high_b: f64) -> f64 {
    let sep1 = (low_a - high_b).max(0.0);
    let sep2 = (low_b - high_a).max(0.0);
    let sep = sep1.max(sep2);
    sep
}

impl Road {
    pub fn new(params: Rc<Parameters>) -> Self {
        let ego_car = Car::new(&params, 0, 0);

        let mut road = Self {
            t: 0.0,
            timesteps: 0,
            last_ego: ego_car.clone(),
            cars: vec![ego_car],
            belief: None,
            cost: Cost::new(1.0),
            ego_is_safe: true,
            debug: !params.run_fast,
            car_traces: Some(Vec::new()),
            trajectory_buffer: Vec::new(),
            params,
        };

        road.cars[0].preferred_vel = SPEED_DEFAULT;
        road.cars[0].theta = PI / 16.0;
        road.cars[0].y = Road::get_lane_y(0);
        // road.set_ego_policy(make_policy_choices()[4].clone());

        road
    }

    // fn double_borrow_mut(&mut self, i1: usize, i2: usize) -> (&mut Car, &mut Car) {
    //     assert_ne!(i1, i2);
    //     if i1 < i2 {
    //         let rem = &mut self.cars[i1..];
    //         let (first, second) = rem.split_at_mut(i2 - i1);
    //         (&mut first[0], &mut second[0])
    //     } else {
    //         let rem = &mut self.cars[i2..];
    //         let (second, first) = rem.split_at_mut(i1 - i2);
    //         (&mut first[0], &mut second[0])
    //     }
    // }

    pub fn add_random_car(&mut self, rng: &Rc<RefCell<StdRng>>) {
        for _ in 0..100 {
            let car = Car::random_new(&self.params, self.cars.len(), rng);
            if self.collides_any_car(&car) {
                continue;
            }
            self.cars.push(car);
            return;
        }
        panic!();
    }

    pub fn add_obstacle(&mut self, x: f64, lane_i: i32) {
        let mut car = Car::new(&self.params, self.cars.len(), lane_i);
        car.x = x;
        car.y += LANE_WIDTH / 4.0;
        car.theta = PI / 2.0;
        car.vel = 0.0;
        car.preferred_vel = 0.0;
        car.crashed = true;

        self.cars.push(car);
    }

    pub fn init_belief(&mut self) {
        let n_policies = make_policy_choices(&self.params).len();
        self.belief = Some(Rc::new(Belief::uniform(self.cars.len(), n_policies)));
    }

    pub fn update_belief(&mut self) {
        let mut belief_rc = self.belief.take().unwrap();
        let belief = Rc::get_mut(&mut belief_rc).expect("update_belief should only be called when it has exclusive access to the top-level road");
        belief.update(&self);
        self.belief = Some(belief_rc);
    }

    pub fn clone_without_cars(&self) -> Self {
        Self {
            params: self.params.clone(),
            t: self.t,
            timesteps: self.timesteps,
            cars: Vec::new(),
            belief: self.belief.clone(),
            last_ego: self.last_ego.clone(),
            cost: self.cost.clone(),
            ego_is_safe: self.ego_is_safe,
            car_traces: None,
            trajectory_buffer: Vec::new(),
            debug: self.debug,
        }
    }

    pub fn sim_estimate(&self) -> Self {
        let mut road = self.clone_without_cars();
        road.cars = self.cars.iter().map(|c| c.sim_estimate()).collect();
        // preserve the ego-car
        road.cars[0] = self.cars[0].clone();
        road.debug = false;
        road.cost.discount_factor = self.params.cost.discount_factor;
        road
    }

    pub fn open_loop_estimate(&self, keep_car_i: usize) -> Self {
        let mut road = self.clone_without_cars();
        road.cars = self.cars.iter().map(|c| c.open_loop_estimate()).collect();
        // preserve the ego-car and the specified one
        road.cars[0] = self.cars[0].clone();
        if keep_car_i != 0 {
            road.cars[keep_car_i] = self.cars[keep_car_i].clone();
        }
        road.debug = false;
        road.cost.discount_factor = self.params.cost.discount_factor;
        road
    }

    pub fn sample_belief(&self, rng: &mut StdRng) -> Self {
        let belief = self.belief.clone().unwrap();
        let policies = make_obstacle_vehicle_policy_choices(&self.params);

        let mut road = self.sim_estimate();

        let sample = belief.sample(rng);

        // sample policies from the belief state
        for (car_i, car) in road.cars.iter_mut().enumerate().skip(1) {
            car.side_policy = Some(policies[sample[car_i]].clone());
        }

        road
    }

    pub fn ego_policy(&self) -> &SidePolicy {
        self.cars[0].side_policy.as_ref().unwrap()
    }

    pub fn set_ego_policy(&mut self, policy: SidePolicy) {
        // we don't want to lose state when "switching" to the same policy we are already running
        // this only really matters for mantain velocity policy and delayed switch policies
        // For the delayed switching policies, it is assumed that any switch is wanted
        // in order to reset the switching time
        let policy_id = policy.policy_id();
        let old_policy_id = self.ego_policy().policy_id();
        if policy_id < 100 && policy_id == old_policy_id {
            // not a delayed policy... keep state by not changing
            return;
        }
        self.cars[0].side_policy = Some(policy);
    }

    pub fn take_update_steps(&mut self, t: f64, dt: f64) {
        // For example, w/ t = 1.0, dt = 0.4 we get steps [0.2, 0.4, 0.4]
        let n_full_steps = (t / dt).floor() as i32;
        let remaining = t - dt * n_full_steps as f64;
        if remaining > 1e-6 {
            self.update(remaining);
        }
        for _ in 0..n_full_steps {
            self.update(dt);
        }
    }

    pub fn super_debug(&self) -> bool {
        self.debug
            && self.params.super_debug
            && self.timesteps + self.params.debug_steps_before >= self.params.max_steps as usize
    }

    pub fn lane_definitely_clear_between(
        &self,
        skip_car_i: usize,
        lane_i: i32,
        low_x: f64,
        high_x: f64,
    ) -> bool {
        assert!(low_x < high_x);
        for c in self.cars.iter() {
            if c.car_i == skip_car_i {
                continue;
            }
            if c.x + c.length / 2.0 < low_x || c.x - c.length / 2.0 > high_x {
                continue;
            }
            let small_theta = c.theta.abs() < 5.0 / 180.0 * PI;
            if c.current_lane() != lane_i && small_theta {
                continue;
            }
            if small_theta {
                return false;
            }

            // larger theta... more complicated case!
            if parry2d_f64::query::intersection_test(
                &Isometry::translation((high_x + low_x) * 0.5, Road::get_lane_y(lane_i)),
                &parry2d_f64::shape::Cuboid::new(vector!((high_x - low_x) * 0.5, LANE_WIDTH * 0.5)),
                &c.pose(),
                &c.shape(),
            )
            .unwrap()
            {
                return false;
            }
        }
        true
    }

    pub fn collides_between(&self, car_i1: usize, car_i2: usize) -> bool {
        assert_ne!(car_i1, car_i2);

        let car_a = &self.cars[car_i1];
        let car_b = &self.cars[car_i2];

        if (car_a.x - car_b.x).abs() > (car_a.length + car_b.length) / 2.0 {
            return false;
        }

        parry2d_f64::query::intersection_test(
            &car_a.pose(),
            &car_a.shape(),
            &car_b.pose(),
            &car_b.shape(),
        )
        .unwrap()
    }

    #[allow(unused)]
    pub fn collides_any(&self, car_i: usize) -> bool {
        let car = &self.cars[car_i];
        let pose = car.pose();
        let shape = car.shape();
        for (i, c) in self.cars.iter().enumerate() {
            if i == car_i {
                continue;
            }

            if parry2d_f64::query::intersection_test(&pose, &shape, &c.pose(), &c.shape()).unwrap()
            {
                return true;
            }
        }
        false
    }

    pub fn collides_any_car(&self, car: &Car) -> bool {
        let pose = car.pose();
        let shape = car.shape();
        for c in self.cars.iter() {
            if parry2d_f64::query::intersection_test(&pose, &shape, &c.pose(), &c.shape()).unwrap()
            {
                return true;
            }
        }
        false
    }

    #[allow(unused)]
    pub fn dist_clear_ahead(&self, car_i: usize) -> Option<(f64, usize)> {
        self.dist_clear::<true>(car_i)
    }

    pub fn dist_clear_ahead_in_lane(&self, car_i: usize, lane_i: i32) -> Option<(f64, usize)> {
        self.dist_clear_in_lane::<true>(car_i, Some(lane_i))
    }

    // fn dist_clear_behind(&self, car_i: usize) -> Option<(f64, usize)> {
    //     self.dist_clear(car_i, false)
    // }

    pub fn dist_clear<const AHEAD: bool>(&self, car_i: usize) -> Option<(f64, usize)> {
        self.dist_clear_in_lane::<AHEAD>(car_i, None)
    }

    pub fn dist_clear_in_lane<const AHEAD: bool>(
        &self,
        car_i: usize,
        lane_i: Option<i32>,
    ) -> Option<(f64, usize)> {
        let car = &self.cars[car_i];

        let mut min_dist = f64::MAX;
        let mut min_car_i = None;

        let dist_thresh = car.vel * 100.0 + car.length;

        let pose = self.cars[car_i].pose();
        let shape = self.cars[car_i].shape();
        // we remove rotation from the ego's aabb calculation because otherwise we will
        // see spurious potential collisions from the back of the car while turning.
        // no rotation just focuses on the front of the ego-car for this calculation
        let no_rotation_pose = if let Some(lane_i) = lane_i {
            Isometry::translation(pose.translation.vector.x, Road::get_lane_y(lane_i))
        } else {
            Isometry::translation(pose.translation.vector.x, pose.translation.vector.y)
        };

        let aabb = shape.compute_aabb(&no_rotation_pose);
        for (i, c) in self.cars.iter().enumerate() {
            // fastest way to get to 'continue' (for performance)
            if (c.x - car.x).abs() >= dist_thresh {
                continue;
            }
            // skip cars behind (or ahead of) this one
            if AHEAD && c.x < car.x || !AHEAD && c.x > car.x {
                // if i == 0 && car_i == 14 {
                //     eprintln_f!("Skipping {car_i} to ego: c.x {:.2} car.x {:.2}", c.x, car.x);
                // }
                continue;
            }
            if i == car_i {
                continue;
            }
            if self.params.obstacles_only_for_ego && c.crashed && !car.is_ego() {
                continue;
            }

            let other_aabb = c.shape().compute_aabb(&c.pose());
            let side_sep = range_dist(
                aabb.mins[1],
                aabb.maxs[1],
                other_aabb.mins[1],
                other_aabb.maxs[1],
            );

            if side_sep <= SIDE_MARGIN {
                let dist = other_aabb.mins[0] - aabb.maxs[0];
                if dist < min_dist {
                    min_dist = dist;
                    min_car_i = Some(i);
                }

                if self.params.separation_debug {
                    if self.super_debug() && car.is_ego() {
                        eprintln_f!("ego from {i} {side_sep=:.2}, {dist=:.2}");
                    } else if self.super_debug()
                        && c.is_ego()
                        && self.params.debug_car_i == Some(car.car_i)
                    {
                        eprintln_f!("{car.car_i} from ego {side_sep=:.2}, {dist=:.2}");
                    }
                }
            }
        }

        Some((min_dist, min_car_i?))
    }

    fn min_unsafe_dist(&self, car_i: usize) -> Option<f64> {
        let safety_margin = self.params.cost.safety_margin;

        let car = &self.cars[car_i];

        let mut min_dist = None;
        let dist_thresh = 2.0 * car.length + safety_margin;

        let pose = car.pose();
        let shape = car.shape();
        let aabb = shape.compute_aabb(&pose);
        for (i, c) in self.cars.iter().enumerate() {
            if i == car_i {
                continue;
            }
            if (c.x - car.x).abs() >= dist_thresh {
                continue;
            }

            let other_aabb = c.shape().compute_aabb(&c.pose());
            let side_sep = range_dist(
                aabb.mins[1],
                aabb.maxs[1],
                other_aabb.mins[1],
                other_aabb.maxs[1],
            );
            if side_sep <= safety_margin {
                let longitidinal_sep = range_dist(
                    aabb.mins[0],
                    aabb.maxs[0],
                    other_aabb.mins[0],
                    other_aabb.maxs[0],
                );
                let dist = side_sep.max(longitidinal_sep);
                if dist < min_dist.unwrap_or(safety_margin) {
                    // if self.super_debug() && car.is_ego() {
                    //     let road = self;
                    //     eprintln_f!("{road.timesteps}: ego from {i}, {car.x=:.2}, {c.x=:.2}, car.length + safety_margin: {:.2} mins: {:.2?} maxs: {:.2?}, other mins: {:.2?} maxs: {:.2?}, {side_sep=:.2}, {dist=:.2}",
                    //                 2.0 * car.length + safety_margin,
                    //                 aabb.mins.coords.as_slice(), aabb.maxs.coords.as_slice(), other_aabb.mins.coords.as_slice(), other_aabb.maxs.coords.as_slice());
                    // }

                    // bounding boxes are close enough, now do the more expensive exact calculation
                    match query::closest_points(&pose, &shape, &c.pose(), &c.shape(), safety_margin)
                    {
                        Ok(ClosestPoints::WithinMargin(a, b)) => {
                            let dist = (a - b).magnitude();
                            if dist < min_dist.unwrap_or(safety_margin) {
                                min_dist = Some(dist);
                            }
                        }
                        Ok(ClosestPoints::Intersecting) => {
                            min_dist = Some(0.0);
                        }
                        _ => (),
                    }
                }
            }
        }

        min_dist
    }

    pub fn update(&mut self, dt: f64) {
        let mut trajectory = std::mem::replace(&mut self.trajectory_buffer, Vec::new());

        for car_i in 0..self.cars.len() {
            if !self.cars[car_i].crashed {
                // policy
                {
                    let mut policy = self.cars[car_i].side_policy.take().unwrap();
                    self.cars[car_i].target_lane_i = policy.choose_target_lane(self, car_i);
                    self.cars[car_i].target_follow_time = policy.choose_follow_time(self, car_i);
                    self.cars[car_i].target_vel = policy.choose_vel(self, car_i);
                    policy.choose_trajectory(self, car_i, &mut trajectory);
                    self.cars[car_i].side_policy = Some(policy);
                }

                // forward control
                {
                    let mut control = self.cars[car_i].forward_control.take().unwrap();
                    let mut accel = control.choose_accel(self, car_i);

                    let car = &mut self.cars[car_i];
                    accel = accel.max(-BREAKING_ACCEL).min(car.preferred_vel);
                    car.vel = (car.vel + accel * dt).max(0.0).min(car.preferred_vel);
                    self.cars[car_i].forward_control = Some(control);
                }

                // side control
                {
                    let mut control = self.cars[car_i].side_control.take().unwrap();
                    let target_steer = control.choose_steer(self, car_i, &trajectory);

                    let car = &mut self.cars[car_i];
                    let target_steer_accel = (target_steer - car.steer) / dt;
                    let steer_accel = target_steer_accel
                        .max(-car.preferred_steer_accel)
                        .min(car.preferred_steer_accel);

                    car.steer = (car.steer + steer_accel * dt)
                        .max(-PRIUS_MAX_STEER)
                        .min(PRIUS_MAX_STEER);
                    self.cars[car_i].side_control = Some(control);
                }
            }
        }

        for car in self.cars.iter_mut() {
            if !car.crashed {
                car.update(dt);
            }
        }

        if self.super_debug() {
            let ego = &self.cars[0];
            eprintln!(
                "{}: ego x: {:.2}, y: {:.2}, vel: {:.10}",
                self.timesteps, ego.x, ego.y, ego.vel
            );
        }

        if let Some(traces) = self.car_traces.as_mut() {
            traces.resize(self.cars.len(), Vec::new());

            for (car_i, car) in self.cars.iter_mut().enumerate() {
                if !car.crashed {
                    let policy_id = car.side_policy.as_ref().unwrap().policy_id();
                    traces[car_i].push((point!(car.x, car.y, car.theta), policy_id));
                }
            }
        }

        if self.params.only_crashes_with_ego {
            let i1 = 0;
            for i2 in 1..self.cars.len() {
                if self.cars[i1].crashed && self.cars[i2].crashed {
                    continue;
                }
                if self.collides_between(i1, i2) {
                    if self.super_debug() {
                        eprintln!();
                        eprintln!("{}: CRASH between:", self.timesteps);
                        eprintln!("{:.2?}", self.cars[i1]);
                        eprintln!("{:.2?}", self.cars[i2]);
                        eprintln!();
                    }

                    self.cars[i1].crashed = true;
                    self.cars[i2].crashed = true;
                }
            }
        } else {
            for (i1, i2) in (0..self.cars.len()).tuple_combinations() {
                if self.cars[i1].crashed && self.cars[i2].crashed {
                    continue;
                }
                if self.collides_between(i1, i2) {
                    if self.super_debug() {
                        eprintln!();
                        eprintln!("{}: CRASH between:", self.timesteps);
                        eprintln!("{:.2?}", self.cars[i1]);
                        eprintln!("{:.2?}", self.cars[i2]);
                        eprintln!();
                    }

                    self.cars[i1].crashed = true;
                    self.cars[i2].crashed = true;
                }
            }
        }

        self.t += dt;
        self.timesteps += 1;

        self.update_cost(dt);

        self.trajectory_buffer = trajectory;
    }

    fn update_cost(&mut self, dt: f64) {
        let cparams = &self.params.cost;
        let car = &self.cars[0];

        if car.vel < car.preferred_vel {
            self.cost.efficiency += cparams.efficiency_weight
                * cparams.efficiency_low_speed_cost
                * (car.preferred_vel - car.vel)
                * dt
                * self.cost.discount;
        } else if car.vel > car.preferred_vel + cparams.efficiency_high_speed_tolerance {
            self.cost.efficiency += cparams.efficiency_weight
                * cparams.efficiency_high_speed_cost
                * (car.vel - car.preferred_vel - cparams.efficiency_high_speed_tolerance)
                * dt
                * self.cost.discount;
        }

        self.cost.efficiency += cparams.efficiency_weight
            * (car.preferred_vel - car.vel).abs()
            * dt
            * self.cost.discount;

        let min_dist = self.min_unsafe_dist(0);
        if min_dist.is_some() {
            self.cost.safety += cparams.safety_weight * dt * self.cost.discount;
            if self.debug {
                eprintln!("{}: UNSAFE: {:.2}", self.timesteps, min_dist.unwrap());
            }
        }
        self.ego_is_safe = min_dist.is_none();

        let policy_id = car.policy_id();
        let last_policy_id = self.last_ego.policy_id();
        if policy_id != last_policy_id {
            self.cost.smoothness += cparams.smoothness_weight * self.cost.discount;
            if self.debug {
                eprintln_f!(
                    "{}: policy change from {last_policy_id} to {policy_id}",
                    self.timesteps
                );
                eprintln!("New policy:\n{:?}", self.ego_policy().operating_policy());
            }
        }

        let accel = (car.vel - self.last_ego.vel) / dt;
        if accel <= -cparams.uncomfortable_dec {
            self.cost.uncomfortable_dec +=
                cparams.uncomfortable_dec_weight * dt * self.cost.discount;
        }
        let curvature_change = (car.theta - self.last_ego.theta).abs() / dt;
        if curvature_change >= cparams.large_curvature_change {
            self.cost.curvature_change += cparams.curvature_change_weight * dt * self.cost.discount;
        }

        self.last_ego = self.cars[0].clone();
        self.cost.update_discount(dt);
    }

    pub fn draw(&self, r: &mut Rvx) {
        // draw a 'road'
        r.draw(
            Rvx::square()
                .scale_xy(&[ROAD_LENGTH, LANE_WIDTH * 2.0])
                .color(RvxColor::GRAY),
        );
        r.draw(
            Rvx::square()
                .scale_xy(&[ROAD_LENGTH, 0.2])
                .translate(&[0.0, -LANE_WIDTH])
                .color(RvxColor::WHITE),
        );
        r.draw(
            Rvx::square()
                .scale_xy(&[ROAD_LENGTH, 0.2])
                .translate(&[0.0, LANE_WIDTH])
                .color(RvxColor::WHITE),
        );

        r.draw(
            Rvx::text(&format!("{}", self.timesteps), "Arial", 150.0)
                .rot(-PI / 2.0)
                .translate(&[0.0, 5.0 * LANE_WIDTH])
                .color(RvxColor::WHITE),
        );

        // adjust for ego car
        r.set_translate_modifier(-self.cars[0].x, 0.0);

        // draw the dashes in the middle
        let dash_interval = ROAD_DASH_LENGTH + ROAD_DASH_DIST;
        let dash_offset = (self.cars[0].x / dash_interval).round() * dash_interval;
        for dash_i in -5..=5 {
            r.draw(
                Rvx::square()
                    .scale_xy(&[ROAD_DASH_LENGTH, 0.2])
                    .translate(&[dash_i as f64 * dash_interval + dash_offset, 0.0])
                    .color(RvxColor::WHITE),
            );
        }

        // draw the cars
        for (i, car) in self.cars.iter().enumerate() {
            if i == 0 && car.crashed {
                car.draw(&self.params, r, RvxColor::ORANGE.set_a(0.6));
            } else if i == 0 {
                car.draw(&self.params, r, RvxColor::GREEN.set_a(0.6));
            } else if car.crashed {
                car.draw(&self.params, r, RvxColor::RED.set_a(0.6));
            } else if car.vel == 0.0 {
                car.draw(&self.params, r, RvxColor::WHITE.set_a(0.6));
            } else {
                car.draw(&self.params, r, RvxColor::BLUE.set_a(0.6));
            }
        }
    }

    pub fn reset_car_traces(&mut self) {
        if self.params.run_fast {
            self.car_traces = None;
        } else {
            self.car_traces = Some(Vec::new());
        }
    }

    pub fn make_traces(&self, depth_level: u32, include_obstacle_cars: bool) -> Vec<rvx::Shape> {
        let mut shapes = Vec::new();

        if self.car_traces.is_none() {
            return shapes;
        }

        // if depth_level != 2 {
        //     return Vec::new();
        // }

        let traces: &Vec<Vec<(Point3<f64>, u32)>> = self.car_traces.as_ref().unwrap();
        for (car_i, trace) in traces.iter().enumerate() {
            if trace.is_empty() {
                continue;
            }

            // sparsify points that are _really_ close together
            let mut points_2d = trace.iter().map(|(p, _)| p).copied().collect_vec();
            let mut p_i = 0;
            while p_i + 1 < points_2d.len() {
                if (points_2d[p_i] - points_2d[p_i + 1]).magnitude_squared() < 0.1f64.powi(2) {
                    points_2d.remove(p_i + 1);
                    continue;
                }
                p_i += 1;
            }

            let points = points_2d
                .iter()
                .flat_map(|p| &p.coords.as_slice()[0..2])
                .copied()
                .collect_vec();

            if car_i == 0 {
                // eprintln!("Points in trace: {}", trace.len());

                let base_line_color = if self.cars[0].crashed {
                    RvxColor::RED.scale_rgb(0.5)
                } else if self.cost.safety > 0.0 {
                    RvxColor::PINK
                } else {
                    RvxColor::GREEN
                };

                let line_color = match depth_level {
                    0 => base_line_color.set_a(0.6),
                    1 => base_line_color.scale_rgb(0.6).set_a(0.6),
                    2 => base_line_color.scale_rgb(0.3).set_a(0.6),
                    3 | _ => base_line_color.scale_rgb(0.1).set_a(0.6),
                };

                let mut line_width = match depth_level {
                    0 => 12.0,
                    1 => 6.0,
                    2 => 3.0,
                    3 | _ => 1.5,
                };
                if self.cars[0].crashed || self.cost.safety > 0.0 {
                    line_width += 4.0;
                }

                shapes.push(Rvx::lines(&points, line_width).color(line_color));

                let dot_color = match self.ego_policy().operating_policy().policy_id() {
                    1 | 3 => RvxColor::RED,
                    4 => RvxColor::BLUE,
                    _ => RvxColor::BLACK,
                };

                shapes.push(Rvx::array(
                    Rvx::circle().scale(0.15).color(dot_color.set_a(0.4)),
                    &points,
                ));

            // label the points with the policy_id active at that point in time
            // for (xyt, policy_id) in trace.iter() {
            //     shapes.push(
            //         Rvx::text(&format!("{}", policy_id), "Arial", 30.0)
            //             .rot(-PI / 2.0)
            //             .translate(&[xyt.x, xyt.y])
            //             .color(RvxColor::BLACK),
            //     );
            // }
            } else if Some(car_i) == self.params.debug_car_i {
                shapes.push(Rvx::lines(&points, 6.0).color(RvxColor::DARK_GRAY.set_a(0.9)));
                // shapes.push(Rvx::array(
                //     Rvx::circle().scale(0.2).color(RvxColor::DARK_GRAY),
                //     &points,
                // ));
            } else if include_obstacle_cars {
                shapes.push(Rvx::lines(&points, 6.0).color(RvxColor::WHITE.set_a(0.5)));
            }

            // let draw_trace = trace[1];
            // let mut draw_car = Car::new(car_i, 0);
            // draw_car.x = draw_trace.x;
            // draw_car.y = draw_trace.y;
            // draw_car.theta = draw_trace.z;
            // if car_i == 0 {
            //     draw_car.draw(r, RvxColor::GREEN.set_a(0.5));
            // } else {
            //     draw_car.draw(r, RvxColor::DARK_GRAY.set_a(0.5));
            // }
        }

        shapes
    }

    pub fn get_lane_y(lane_i: i32) -> f64 {
        (lane_i as f64 - 0.5) * LANE_WIDTH
    }

    pub fn get_lane_i(y: f64) -> i32 {
        (y / LANE_WIDTH + 0.5).round() as i32
    }
}
