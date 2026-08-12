use bevy::{
    asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader},
    mesh::PrimitiveTopology,
    prelude::*,
};
use thiserror::Error;

// Unlike normal objPlugin, we need original verts as well
// AND we need mesh vertex indices to match obj vertex indices for mhclo binding
#[derive(Asset, TypePath, Debug, Clone)]
pub struct ObjBaseMesh {
    pub mesh: Mesh,
    /// The (makehuman/obj) positions in the base mesh
    pub vertices: Vec<Vec3>,
    /// Mesh vertex idx -> obj vertex idx mapping (identity for our loader)
    pub mhid_lookup: Vec<u16>,
}

#[derive(Default, TypePath)]
pub struct ObjBaseMeshLoader;

#[derive(Debug, Error)]
pub enum ObjBaseMeshLoaderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("OBJ parse error: {0}")]
    Obj(#[from] obj::ObjError),
}

impl AssetLoader for ObjBaseMeshLoader {
    type Asset = ObjBaseMesh;
    type Settings = ();
    type Error = ObjBaseMeshLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let obj_data = obj::ObjData::load_buf(&bytes[..])?;

        // MakeHuman uses DECIMETER units - scale to meters (0.1x)
        // Negate X and Z to convert from MH coords to Bevy coords
        const SCALE: f32 = 0.1;

        // Pre-transform obj positions
        let obj_positions: Vec<[f32; 3]> = obj_data
            .position
            .iter()
            .map(|p| [-p[0] * SCALE, p[1] * SCALE, -p[2] * SCALE])
            .collect();

        // Build mesh with proper UV seam handling:
        // Each unique (pos_idx, uv_idx) pair gets its own mesh vertex
        // mhid_lookup tracks which obj vertex each mesh vertex came from
        use std::collections::HashMap;
        let mut vertex_cache: HashMap<(usize, Option<usize>), u32> = HashMap::new();
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut mhid_lookup: Vec<u16> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();

        // Process all objects and groups
        for object in &obj_data.objects {
            for group in &object.groups {
                for poly in &group.polys {
                    let verts: Vec<_> = poly.0.iter().collect();

                    // Triangulate: support triangles and quads
                    let tri_indices: Vec<(usize, usize, usize)> = match verts.len() {
                        3 => vec![(0, 1, 2)],
                        4 => vec![(0, 1, 2), (0, 2, 3)],
                        n if n > 4 => {
                            // Fan triangulation for n-gons
                            (1..n - 1).map(|i| (0, i, i + 1)).collect()
                        }
                        _ => continue,
                    };

                    for (i0, i1, i2) in tri_indices {
                        for &vert_idx in &[i0, i1, i2] {
                            let vert = &verts[vert_idx];
                            let pos_idx = vert.0;
                            let uv_idx = vert.1;
                            let key = (pos_idx, uv_idx);

                            let mesh_idx = *vertex_cache.entry(key).or_insert_with(|| {
                                let idx = positions.len() as u32;
                                positions.push(obj_positions[pos_idx]);
                                uvs.push(match uv_idx {
                                    Some(i) => {
                                        [obj_data.texture[i][0], 1.0 - obj_data.texture[i][1]]
                                    }
                                    None => [0.0, 0.0],
                                });
                                mhid_lookup.push(pos_idx as u16);
                                idx
                            });

                            indices.push(mesh_idx);
                        }
                    }
                }
            }
        }

        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(bevy::mesh::Indices::U32(indices));

        // Compute smooth normals
        mesh.compute_smooth_normals();

        // Store original obj vertices for mhclo fitting
        let vertices: Vec<Vec3> = obj_positions.iter().map(|p| Vec3::from_array(*p)).collect();

        Ok(ObjBaseMesh {
            mesh,
            vertices,
            mhid_lookup,
        })
    }
}
