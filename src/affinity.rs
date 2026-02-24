pub fn pin_current_thread_to(core_index: usize) -> bool {
    let Some(cores) = core_affinity::get_core_ids() else {
        return false;
    };
    let Some(core_id) = cores.into_iter().find(|c| c.id == core_index) else {
        return false;
    };
    core_affinity::set_for_current(core_id)
}
