use card::Card;
use dialoguer::Select;
use drm::{
    buffer::DrmFourcc,
    control::{self, atomic, connector, crtc, property, AtomicCommitFlags, Device as ControlDevice},
    ClientCapability, Device,
};
use std::{fs, thread, time::Duration};

mod card;
mod modeset;

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cards: Vec<String> = fs::read_dir("/dev/dri")?
        .filter_map(|entry| entry.ok().and_then(|e| e.file_name().into_string().ok()))
        .filter(|name| name.starts_with("card"))
        .collect();

    assert!(!cards.is_empty(), "No drm devices found");
    cards.sort();

    println!("Found {} drm devices:\n{:?}", cards.len(), cards);

    let card = if cards.len() == 1 {
        &cards[0]
    } else {
        let index = Select::new()
            .with_prompt("Which gpu would you like to use?")
            .items(&cards)
            .default(0)
            .interact()?;

        &cards[index]
    };

    let card = Card::open(format!("/dev/dri/{card}"));

    card.set_client_capability(ClientCapability::UniversalPlanes, true)?;
    card.set_client_capability(drm::ClientCapability::Atomic, true)?;

    let res = card.resource_handles().expect("Could not load normal resource ids.");
    let coninfo: Vec<connector::Info> = res.connectors().iter().flat_map(|con| card.get_connector(*con, true)).collect();
    let crtcinfo: Vec<crtc::Info> = res.crtcs().iter().flat_map(|crtc| card.get_crtc(*crtc)).collect();

    let con = coninfo
        .iter()
        .find(|&i| i.state() == connector::State::Connected)
        .expect("No connected connectors");

    let &mode = con.modes().first().expect("No modes found on connector");

    let (disp_width, disp_height) = mode.size();

    let crtc = crtcinfo.first().expect("No crtcs found");

    let mut db = card
        .create_dumb_buffer((disp_width.into(), disp_height.into()), DrmFourcc::Xrgb8888, 32)
        .expect("Could not create dumb buffer");

    {
        let mut map = card.map_dumb_buffer(&mut db).expect("Could not map dumbbuffer");

        for b in map.as_mut() {
            *b = 0xFF / 2;
        }
    }

    let fb = card.add_framebuffer(&db, 24, 32).expect("Could not create FB");

    let planes = card.plane_handles().expect("Could not list planes");
    let (better_planes, compatible_planes): (Vec<control::plane::Handle>, Vec<control::plane::Handle>) = planes
        .iter()
        .filter(|&&plane| {
            card.get_plane(plane)
                .map(|plane_info| {
                    let compatible_crtcs = res.filter_crtcs(plane_info.possible_crtcs());
                    compatible_crtcs.contains(&crtc.handle())
                })
                .unwrap_or(false)
        })
        .partition(|&&plane| {
            if let Ok(props) = card.get_properties(plane) {
                for (&id, &val) in props.iter() {
                    if let Ok(info) = card.get_property(id) {
                        if info.name().to_str().map(|x| x == "type").unwrap_or(false) {
                            return val == (drm::control::PlaneType::Primary as u32).into();
                        }
                    }
                }
            }
            false
        });

    let plane = if let Some(plane) = better_planes.first().or_else(|| compatible_planes.first()) {
        *plane
    } else {
        panic!("No suitable planes found");
    };

    let con_props = card.get_properties(con.handle())?.as_hashmap(&card)?;
    let crtc_props = card.get_properties(crtc.handle())?.as_hashmap(&card)?;
    let plane_props = card.get_properties(plane)?.as_hashmap(&card)?;

    let mut atomic_req = atomic::AtomicModeReq::new();
    atomic_req.add_property(
        con.handle(),
        con_props["CRTC_ID"].handle(),
        property::Value::CRTC(Some(crtc.handle())),
    );
    let blob = card.create_property_blob(&mode).expect("Failed to create blob");
    atomic_req.add_property(crtc.handle(), crtc_props["MODE_ID"].handle(), blob);
    atomic_req.add_property(crtc.handle(), crtc_props["ACTIVE"].handle(), property::Value::Boolean(true));
    atomic_req.add_property(plane, plane_props["FB_ID"].handle(), property::Value::Framebuffer(Some(fb)));
    atomic_req.add_property(plane, plane_props["CRTC_ID"].handle(), property::Value::CRTC(Some(crtc.handle())));
    atomic_req.add_property(plane, plane_props["SRC_X"].handle(), property::Value::UnsignedRange(0));
    atomic_req.add_property(plane, plane_props["SRC_Y"].handle(), property::Value::UnsignedRange(0));
    atomic_req.add_property(
        plane,
        plane_props["SRC_W"].handle(),
        property::Value::UnsignedRange(u64::from(mode.size().0) << 16),
    );
    atomic_req.add_property(
        plane,
        plane_props["SRC_H"].handle(),
        property::Value::UnsignedRange(u64::from(mode.size().1) << 16),
    );
    atomic_req.add_property(plane, plane_props["CRTC_X"].handle(), property::Value::SignedRange(0));
    atomic_req.add_property(plane, plane_props["CRTC_Y"].handle(), property::Value::SignedRange(0));
    atomic_req.add_property(
        plane,
        plane_props["CRTC_W"].handle(),
        property::Value::UnsignedRange(u64::from(mode.size().0)),
    );
    atomic_req.add_property(
        plane,
        plane_props["CRTC_H"].handle(),
        property::Value::UnsignedRange(u64::from(mode.size().1)),
    );

    card.atomic_commit(AtomicCommitFlags::ALLOW_MODESET, atomic_req)
        .expect("Failed to set display mode");

    // let gbm = gbm::Device::new(&card).expect("Failed to create gbm device");
    // let mut bo = gbm
    //     .create_buffer_object::<()>(
    //         mode.size().0.into(),
    //         mode.size().1.into(),
    //         gbm::Format::Xrgb8888,
    //         BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
    //     )
    //     .expect("Failed to create gbm buffer object");

    // let buffer = {
    //     let mut buffer = Vec::new();
    //     for i in 0..mode.size().0 {
    //         for _ in 0..mode.size().1 {
    //             buffer.push(if i % 2 == 0 { 0 } else { 255 });
    //         }
    //     }
    //     buffer
    // };

    // bo.write(&buffer)?;

    // let gbm_fb = gbm.add_framebuffer(&bo, 32, 32)?;
    // gbm.set_crtc(crtc.handle(), Some(gbm_fb), (0, 0), &[con.handle()], Some(mode))
    //     .expect("Failed to set gbm crtc");

    thread::sleep(Duration::from_secs_f64(3.5));

    // gbm.destroy_framebuffer(gbm_fb)?;
    card.destroy_dumb_buffer(db)?;
    card.destroy_framebuffer(fb)?;

    Ok(())
}
