//! Node hierarchy, local transforms and world space evaluation.

use anyhow::{Result, bail};
use glam::{Mat3, Mat4, Quat, Vec3};
use serde_json::Value;

const POLAR_ITERATIONS: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    pub fn from_matrix(matrix: Mat4) -> Self {
        let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
        Self {
            translation,
            rotation: rotation.normalize(),
            scale,
        }
    }
}

pub struct Scene {
    pub names: Vec<String>,
    pub parents: Vec<Option<usize>>,
    pub defaults: Vec<Transform>,
    /// Every node, parents before their children.
    pub order: Vec<usize>,
}

impl Scene {
    pub fn new(json: &Value) -> Result<Self> {
        let nodes = json
            .get("nodes")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let count = nodes.len();
        let mut names = Vec::with_capacity(count);
        let mut parents = vec![None; count];
        let mut children = vec![Vec::new(); count];
        let mut defaults = Vec::with_capacity(count);
        for (index, node) in nodes.iter().enumerate() {
            let name = node.get("name").and_then(Value::as_str);
            names.push(
                name.map(str::to_owned)
                    .unwrap_or_else(|| format!("node_{index}")),
            );
            defaults.push(read_transform(node));
            for child in node
                .get("children")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(child) = child
                    .as_u64()
                    .map(|child| child as usize)
                    .filter(|&child| child < count)
                else {
                    bail!("node {index} has an invalid child");
                };
                if parents[child].is_some() {
                    bail!("node {child} has more than one parent");
                }
                parents[child] = Some(index);
                children[index].push(child);
            }
        }
        let mut order = Vec::with_capacity(count);
        let mut stack: Vec<usize> = (0..count)
            .rev()
            .filter(|&node| parents[node].is_none())
            .collect();
        while let Some(node) = stack.pop() {
            order.push(node);
            stack.extend(children[node].iter().rev());
        }
        if order.len() != count {
            bail!("node hierarchy contains a cycle");
        }
        Ok(Self {
            names,
            parents,
            defaults,
            order,
        })
    }

    pub fn world_matrices(&self, locals: &[Transform]) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.names.len()];
        for &node in &self.order {
            let local = locals[node].matrix();
            world[node] = match self.parents[node] {
                Some(parent) => world[parent] * local,
                None => local,
            };
        }
        world
    }

    pub fn world_rotations(&self, locals: &[Transform]) -> Vec<Quat> {
        self.world_matrices(locals)
            .into_iter()
            .map(rotation_of)
            .collect()
    }
}

/// Closest rotation to a matrix's linear part, so sheared bones under non-uniform scale still get an orientation.
pub fn rotation_of(matrix: Mat4) -> Quat {
    let mut basis = Mat3::from_mat4(matrix);
    let determinant = basis.determinant();
    if determinant.abs() < f32::EPSILON {
        return Quat::IDENTITY;
    }
    if determinant < 0.0 {
        basis.x_axis = -basis.x_axis;
    }
    basis *= determinant.abs().cbrt().recip();
    for _ in 0..POLAR_ITERATIONS {
        let next = (basis + basis.inverse().transpose()) * 0.5;
        let converged = next.abs_diff_eq(basis, 1e-7);
        basis = next;
        if converged {
            break;
        }
    }
    Quat::from_mat3(&basis).normalize()
}

fn read_transform(node: &Value) -> Transform {
    let floats = |key: &str| -> Option<Vec<f32>> {
        let array = node.get(key)?.as_array()?;
        Some(
            array
                .iter()
                .map(|value| value.as_f64().unwrap_or(0.0) as f32)
                .collect(),
        )
    };
    if let Some(matrix) = floats("matrix").filter(|matrix| matrix.len() == 16) {
        return Transform::from_matrix(Mat4::from_cols_slice(&matrix));
    }
    let vector = |key: &str, default: Vec3| {
        floats(key)
            .filter(|v| v.len() == 3)
            .map(|v| Vec3::from_slice(&v))
            .unwrap_or(default)
    };
    let rotation = floats("rotation")
        .filter(|v| v.len() == 4)
        .map(|v| Quat::from_slice(&v).normalize())
        .unwrap_or(Quat::IDENTITY);
    Transform {
        translation: vector("translation", Vec3::ZERO),
        rotation,
        scale: vector("scale", Vec3::ONE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn orders_parents_first_and_composes_world() {
        let json = json!({"nodes": [
            {"name": "child", "translation": [0.0, 1.0, 0.0]},
            {"name": "root", "children": [0], "translation": [1.0, 0.0, 0.0], "scale": [2.0, 2.0, 2.0]},
        ]});
        let scene = Scene::new(&json).unwrap();
        assert_eq!(scene.order, vec![1, 0]);
        let world = scene.world_matrices(&scene.defaults);
        assert!(
            world[0]
                .w_axis
                .truncate()
                .abs_diff_eq(Vec3::new(1.0, 2.0, 0.0), 1e-6)
        );
    }

    #[test]
    fn rotation_of_ignores_scale_and_shear() {
        let rotation = Quat::from_rotation_y(0.6);
        let scaled =
            Mat4::from_scale_rotation_translation(Vec3::new(0.01, 0.01, 0.01), rotation, Vec3::ONE);
        assert!(rotation_of(scaled).abs_diff_eq(rotation, 1e-5));
        let sheared = Mat4::from_quat(rotation)
            * Mat4::from_scale(Vec3::new(1.0, 0.8, 1.1))
            * Mat4::from_quat(Quat::from_rotation_x(0.3));
        let closest = rotation_of(sheared);
        assert!((closest.length() - 1.0).abs() < 1e-5);
        assert!(closest.angle_between(rotation * Quat::from_rotation_x(0.3)) < 0.1);
    }

    #[test]
    fn rejects_multiple_parents() {
        let json = json!({"nodes": [{"name": "a"}, {"children": [0]}, {"children": [0]}]});
        assert!(Scene::new(&json).is_err());
    }
}
