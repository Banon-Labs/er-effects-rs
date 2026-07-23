//! Assemble a fully-textured object: FLVER geometry + resolved materials + decoded
//! TPF textures, ready for a renderer to consume. This is the M3 "integrated object"
//! data product (everything but the live engine cbuffers).

use crate::flver::ObjectMesh;
use crate::material::ResolvedMaterial;
use crate::texture::DecodedTexture;
use crate::trace::{ObjectCategory, ObjectRef};

/// The textures bound to one mesh's material, by role.
#[derive(Default, Clone)]
pub struct MeshTextures {
    pub albedo: Option<DecodedTexture>,
    pub normal: Option<DecodedTexture>,
    pub metallic: Option<DecodedTexture>,
}

/// One mesh plus its resolved material + bound textures.
pub struct TexturedMesh {
    pub mesh: ObjectMesh,
    pub material_name: String,
    pub shader_name: Option<String>,
    pub textures: MeshTextures,
}

pub struct TexturedObject {
    pub label: String,
    pub meshes: Vec<TexturedMesh>,
    pub bounding_box: ([f32; 3], [f32; 3]),
}

impl TexturedObject {
    pub fn textured_mesh_count(&self) -> usize {
        self.meshes
            .iter()
            .filter(|m| m.textures.albedo.is_some())
            .count()
    }

    /// Geometry-only object (no material/texture resolution) from a parsed FLVER.
    pub fn from_model(model: crate::flver::ObjectModel, label: impl Into<String>) -> Self {
        let bounding_box = model.bounding_box;
        let meshes = model
            .meshes
            .into_iter()
            .map(|mesh| TexturedMesh {
                material_name: String::new(),
                shader_name: None,
                textures: MeshTextures::default(),
                mesh,
            })
            .collect();
        Self {
            label: label.into(),
            meshes,
            bounding_box,
        }
    }
}

/// Load a character fully textured: extract (if needed) its FLVER, the matbin corpus,
/// and its high-res texture bundle, then join them.
pub fn load_textured_character(id: &str) -> Result<TexturedObject, String> {
    let object = ObjectRef {
        category: ObjectCategory::Character,
        model: id.to_owned(),
    };
    let model = crate::loader::load_model(&object).map_err(|e| e.to_string())?;
    let matbin_dir = crate::loader::ensure_matbin_corpus().map_err(|e| e.to_string())?;
    let resolved = crate::material::resolve(&model, &matbin_dir, id).map_err(|e| e.to_string())?;

    let tex_dir = crate::loader::ensure_character_textures(id).map_err(|e| e.to_string())?;
    let textures = crate::texture::load_texture_dir(&tex_dir).map_err(|e| e.to_string())?;

    let meshes = model
        .meshes
        .into_iter()
        .map(|mesh| {
            let mat = resolved.get(mesh.material_index);
            let bind = |role_path: Option<&str>| {
                role_path
                    .map(crate::texture::texture_leaf)
                    .and_then(|leaf| textures.get(&leaf).cloned())
            };
            let tex = match mat {
                Some(m) => MeshTextures {
                    albedo: bind(m.albedo()),
                    normal: bind(sampler_by_role(m, "normal")),
                    metallic: bind(sampler_by_role(m, "metallic")),
                },
                None => MeshTextures::default(),
            };
            TexturedMesh {
                material_name: mat.map(|m| m.name.clone()).unwrap_or_default(),
                shader_name: mat.and_then(|m| m.shader_name.clone()),
                textures: tex,
                mesh,
            }
        })
        .collect();

    Ok(TexturedObject {
        label: format!("chr {id}"),
        meshes,
        bounding_box: model.bounding_box,
    })
}

/// First texture path whose sampler name hints the given role keyword.
fn sampler_by_role<'a>(m: &'a ResolvedMaterial, role: &str) -> Option<&'a str> {
    m.textures
        .iter()
        .find(|(n, _)| n.to_lowercase().contains(role))
        .map(|(_, p)| p.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end M3 data product on real c4800 assets (when cached).
    #[test]
    fn real_c4800_textured_object_if_present() {
        // Only run when the FLVER + matbin + tex extractions already exist (no new
        // extraction in tests).
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/er-objectkit");
        if !root.join("character-c4800").exists()
            || !root.join("matbin").exists()
            || !root.join("character-c4800-tex").exists()
        {
            eprintln!("skip: c4800 assets not fully extracted");
            return;
        }
        let obj = load_textured_character("c4800").expect("load textured");
        let textured = obj.textured_mesh_count();
        eprintln!(
            "c4800 textured object: {} meshes, {} with albedo",
            obj.meshes.len(),
            textured
        );
        assert!(textured > 0, "no mesh got an albedo texture");
        // Spot-check a mesh's albedo is a real decoded image.
        let a = obj
            .meshes
            .iter()
            .find_map(|m| m.textures.albedo.as_ref())
            .unwrap();
        assert!(a.width >= 256 && a.rgba.len() == (a.width * a.height * 4) as usize);
    }
}
