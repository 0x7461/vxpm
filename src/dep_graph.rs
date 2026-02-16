use std::collections::{HashMap, HashSet, VecDeque};

use crate::package::Package;

pub struct DepGraph {
    /// package name -> set of custom packages it depends on
    pub forward: HashMap<String, HashSet<String>>,
    /// package name -> set of custom packages that depend on it
    pub reverse: HashMap<String, HashSet<String>>,
}

impl DepGraph {
    /// Build a dependency graph filtered to only inter-custom-package edges.
    pub fn build(packages: &[Package]) -> Self {
        let custom_names: HashSet<String> = packages.iter().map(|p| p.name.clone()).collect();
        let mut forward: HashMap<String, HashSet<String>> = HashMap::new();
        let mut reverse: HashMap<String, HashSet<String>> = HashMap::new();

        for pkg in packages {
            let mut deps = HashSet::new();
            for dep_name in pkg
                .makedepends
                .iter()
                .chain(pkg.hostmakedepends.iter())
                .chain(pkg.depends.iter())
            {
                // Strip -devel suffix to match base package names
                let base = dep_name
                    .strip_suffix("-devel")
                    .unwrap_or(dep_name)
                    .to_string();
                if custom_names.contains(&base) && base != pkg.name {
                    deps.insert(base.clone());
                    reverse
                        .entry(base)
                        .or_default()
                        .insert(pkg.name.clone());
                }
            }
            forward.insert(pkg.name.clone(), deps);
        }

        // Ensure all packages have entries
        for pkg in packages {
            forward.entry(pkg.name.clone()).or_default();
            reverse.entry(pkg.name.clone()).or_default();
        }

        DepGraph { forward, reverse }
    }

    /// Topological sort of all packages (for build order).
    pub fn topological_sort(&self) -> Vec<String> {
        // in_degree[x] = number of custom deps x has
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for (name, deps) in &self.forward {
            in_degree.insert(name.clone(), deps.len());
        }

        let mut queue: VecDeque<String> = VecDeque::new();
        for (name, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(name.clone());
            }
        }

        let mut result = Vec::new();
        while let Some(name) = queue.pop_front() {
            result.push(name.clone());
            if let Some(dependents) = self.reverse.get(&name) {
                for dep in dependents {
                    if let Some(deg) = in_degree.get_mut(dep) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            queue.push_back(dep.clone());
                        }
                    }
                }
            }
        }

        result
    }

    /// Get the set of packages that need rebuilding if `changed` packages are updated.
    /// BFS through reverse dependencies.
    pub fn rebuild_set(&self, changed: &[String]) -> Vec<String> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        for name in changed {
            queue.push_back(name.clone());
            visited.insert(name.clone());
        }

        while let Some(name) = queue.pop_front() {
            if let Some(dependents) = self.reverse.get(&name) {
                for dep in dependents {
                    if visited.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        // Return in topological order, excluding the original changed packages
        let topo = self.topological_sort();
        let changed_set: HashSet<String> = changed.iter().cloned().collect();
        topo.into_iter()
            .filter(|n| visited.contains(n) && !changed_set.contains(n))
            .collect()
    }

    /// Get tree of reverse dependencies for a package (for tree view).
    pub fn reverse_dep_tree(&self, name: &str) -> Vec<TreeNode> {
        self.build_tree(name, &mut HashSet::new())
    }

    fn build_tree(&self, name: &str, visited: &mut HashSet<String>) -> Vec<TreeNode> {
        if visited.contains(name) {
            return vec![];
        }
        visited.insert(name.to_string());

        let mut children = Vec::new();
        if let Some(dependents) = self.reverse.get(name) {
            let mut sorted: Vec<&String> = dependents.iter().collect();
            sorted.sort();
            for dep in sorted {
                let subtree = self.build_tree(dep, visited);
                children.push(TreeNode {
                    name: dep.clone(),
                    children: subtree,
                });
            }
        }
        children
    }
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub name: String,
    pub children: Vec<TreeNode>,
}
