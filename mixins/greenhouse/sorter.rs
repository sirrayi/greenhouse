// Replaces `Sorter::sort` in crates/horizon-client-render/src/gfx/translucency.rs.
//
// Compiled as a local function inside the target, so `Sorter` and its private
// fields (`order`, `indices`) are in scope. The signature mirrors the original:
// the returned slice borrows from the sorter, which is why the lifetime is tied
// to it and not to `centers`.
fn sort<'a>(
    sorter: &'a mut Sorter,
    centers: &[[f32; 3]],
    camera: [f32; 3],
) -> &'a [u32] {
    if greenhouse::on(greenhouse::Feature::SortUnstable) {
        greenhouse::sort_quads(&mut sorter.order, &mut sorter.indices, centers, camera);
    } else {
        // The original, kept so the switch is a real switch and not a claim.
        sorter.order.clear();
        sorter.order.extend(centers.iter().enumerate().map(|(i, c)| {
            let [x, y, z] = std::array::from_fn(|j| camera[j] - c[j]);
            (i as u32, x * x + y * y + z * z)
        }));
        sorter.order.sort_by(|a, b| b.1.total_cmp(&a.1));
        sorter.indices.clear();
        for &(quad, _) in &sorter.order {
            let v = quad * 4;
            sorter.indices.extend([v, v + 1, v + 2, v + 2, v + 3, v]);
        }
    }
    &sorter.indices
}
