//! Uses the sample save in the repository root if it is present.

use std::path::Path;

use metro_save::Save;

fn sample() -> Option<Vec<u8>> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("2033_auto_save")).ok()
}

#[test]
fn reencode_is_lossless() {
    let Some(bytes) = sample() else { return };
    let save = Save::from_bytes(&bytes).unwrap();
    let again = Save::from_bytes(&save.to_bytes()).unwrap();
    for (a, b) in save.container.sections.iter().zip(&again.container.sections) {
        assert_eq!(a.data, b.data);
    }
    assert_eq!(save.entity_ids().len(), 2995);
}

#[test]
fn scripts_decode_with_state() {
    let Some(bytes) = sample() else { return };
    let save = Save::from_bytes(&bytes).unwrap();
    let mut scripts = 0;
    for id in save.entity_ids() {
        if save.is_visual_script(id) {
            let view = save.script(id).expect("visual script parses");
            assert!(view.script.exact);
            let states = view.states.expect("runtime state found");
            assert_eq!(states.ranges.len(), view.script.blocks.len());
            scripts += 1;
        }
    }
    assert_eq!(scripts, 49);
}

#[test]
fn edit_and_reload() {
    let Some(bytes) = sample() else { return };
    let mut save = Save::from_bytes(&bytes).unwrap();
    let id = save.find("vs_room_scene").unwrap();
    save.arm_delay(id, 27, 100).unwrap();
    let reloaded = Save::from_bytes(&save.to_bytes()).unwrap();
    assert_eq!(reloaded.block_state(id, 27).unwrap(), vec![1, 100, 0, 0, 0]);
}
