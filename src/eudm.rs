use rand::prelude::StdRng;

use crate::{
    arg_parameters::Parameters,
    cost::Cost,
    delayed_policy::DelayedPolicy,
    mpdm::make_policy_choices,
    road::Road,
    road_set::RoadSet,
    side_policies::{SidePolicy, SidePolicyTrait},
};

fn dcp_tree_search(
    params: &Parameters,
    policy_choices: &[SidePolicy],
    roads: RoadSet,
    debug: bool,
) -> (SidePolicy, Vec<rvx::Shape>) {
    let mut traces = Vec::new();

    let unchanged_policy = roads.ego_policy();
    let operating_policy = unchanged_policy.operating_policy();
    let eudm = &params.eudm;

    if debug {
        eprintln!(
            "{}: EUDM DCP-Tree search policies and costs, starting with policy {}",
            roads.timesteps(),
            unchanged_policy.policy_id(),
        );
        eprintln!(
            "Starting from base costs: {:7.2?} = {:7.2}",
            roads.cost(),
            roads.cost().total()
        );
    }

    let mut best_sub_policy = None;
    let mut best_cost = Cost::max_value();

    // Let's first consider the ongoing policy, which may be mid-way through a transition
    // unlike everything else we will consider, which won't transition policies for at least some period
    {
        let mut ongoing_roads = roads.clone();
        for depth_level in 0..eudm.search_depth {
            ongoing_roads.reset_car_traces();
            ongoing_roads.take_update_steps(eudm.layer_t, eudm.dt);
            traces.append(&mut ongoing_roads.make_traces(depth_level, false));
        }
        let cost = ongoing_roads.cost();
        if debug {
            let unchanged_policy_id = unchanged_policy.policy_id();
            eprintln_f!(
                "Unchanged: {unchanged_policy_id}: {:7.2?} = {:7.2}",
                cost,
                cost.total()
            );
        }
        if cost < best_cost {
            best_cost = cost;
            best_sub_policy = None;
        }
    }

    let mut init_policy_roads = roads.clone();
    init_policy_roads.set_ego_policy(&operating_policy);

    for switch_depth in 1..=eudm.search_depth {
        init_policy_roads.reset_car_traces();
        init_policy_roads.take_update_steps(eudm.layer_t, eudm.dt);
        traces.append(&mut init_policy_roads.make_traces(switch_depth - 1, false));

        if switch_depth == eudm.search_depth {
            if debug {
                eprintln_f!(
                    "{switch_depth=}: {operating_policy:?}: {:7.2?} = {:7.2}",
                    init_policy_roads.cost(),
                    init_policy_roads.cost().total()
                );
            }

            let cost = init_policy_roads.cost();
            if cost < best_cost {
                best_cost = cost;
                best_sub_policy = Some(&operating_policy);
            }
        } else {
            for (i, sub_policy) in policy_choices.iter().enumerate() {
                let mut roads = init_policy_roads.clone();
                if sub_policy.policy_id() == operating_policy.policy_id() {
                    continue;
                }
                roads.set_ego_policy(sub_policy);

                for depth_level in switch_depth..eudm.search_depth {
                    roads.reset_car_traces();
                    roads.take_update_steps(eudm.layer_t, eudm.dt);
                    traces.append(&mut roads.make_traces(depth_level, false));
                }

                if debug {
                    eprintln_f!(
                        "{switch_depth=} to {i}: {sub_policy:?}: {:7.2?} = {:7.2}",
                        roads.cost(),
                        roads.cost().total()
                    );
                }

                let cost = roads.cost();
                if cost < best_cost {
                    best_cost = cost;
                    if switch_depth == 1 {
                        best_sub_policy = Some(sub_policy);
                    } else {
                        best_sub_policy = Some(&operating_policy);
                    }
                }
            }
        }
    }

    // will be Some if we should switch policies after one layer, and None to stay the same
    if let Some(best_sub_policy) = best_sub_policy {
        (
            SidePolicy::DelayedPolicy(DelayedPolicy::new(
                operating_policy.clone(),
                best_sub_policy.clone(),
                eudm.layer_t,
            )),
            traces,
        )
    } else {
        (unchanged_policy.clone(), traces)
    }
}

pub fn dcp_tree_choose_policy(
    params: &Parameters,
    true_road: &Road,
    rng: &mut StdRng,
) -> (SidePolicy, Vec<rvx::Shape>) {
    let roads = RoadSet::new_samples(true_road, rng, params.eudm.samples_n);
    let debug = true_road.debug
        && true_road.timesteps + params.debug_steps_before >= params.max_steps as usize;
    let policy_choices = make_policy_choices();
    dcp_tree_search(params, &policy_choices, roads, debug)
}