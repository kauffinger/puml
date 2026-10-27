use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;

pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub psr4: Vec<Psr4Root>,
    pub vendor: VendorIndex,
    /// `extra.laravel.dont-discover`: packages whose service providers Laravel does not register.
    pub dont_discover: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Psr4Root {
    pub prefix: String,
    pub dirs: Vec<PathBuf>,
}

pub struct VendorPackage {
    pub name: String,
    pub psr4: Vec<Psr4Root>,
    /// `extra.laravel.providers`, which Laravel's package discovery registers.
    pub providers: Vec<String>,
}

#[derive(Default)]
pub struct VendorIndex {
    pub packages: Vec<VendorPackage>,
}

impl Project {
    pub fn load(root: &Path, include_dev: bool) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|error| format!("{}: {error}", root.display()))?;
        let composer = read_json(&root.join("composer.json"))?;

        Ok(Project::with_composer(root, &composer, include_dev))
    }

    /// The project as described by a composer.json that is not necessarily the one on disk, such as the
    /// version at a pull request's base. Vendor packages always come from the vendor/ directory on disk.
    pub fn with_composer(root: PathBuf, composer: &Value, include_dev: bool) -> Self {
        let name = composer["name"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| root.file_name().unwrap_or_default().to_string_lossy().into_owned());

        let mut psr4 = psr4_roots(&composer["autoload"]["psr-4"], &root);

        if include_dev {
            psr4.extend(psr4_roots(&composer["autoload-dev"]["psr-4"], &root));
        }

        let vendor = VendorIndex::load(&vendor_dir(&root));
        let dont_discover = composer["extra"]["laravel"]["dont-discover"]
            .as_array()
            .map(|names| names.iter().filter_map(|name| name.as_str().map(str::to_string)).collect())
            .unwrap_or_default();

        Project { root, name, psr4, vendor, dont_discover }
    }

    pub fn php_files(&self) -> Vec<PathBuf> {
        let mut files = BTreeSet::new();

        for root in &self.psr4 {
            for dir in &root.dirs {
                collect_php_files(dir, &mut files);
            }
        }

        collect_php_files(&self.root.join("routes"), &mut files);

        let bootstrap = self.root.join("bootstrap/app.php");

        if bootstrap.is_file() {
            files.insert(bootstrap.canonicalize().unwrap_or(bootstrap));
        }

        files.into_iter().collect()
    }

    /// Whether a PHP file belongs to the graph, which reads the files under the PSR-4 roots and the files
    /// Laravel boots from: bootstrap/app.php and routes/.
    pub fn owns(&self, path: &Path) -> bool {
        path.extension().is_some_and(|extension| extension == "php")
            && (self.psr4.iter().flat_map(|root| root.dirs.iter()).any(|dir| path.starts_with(dir))
                || path.starts_with(self.root.join("routes"))
                || path == self.root.join("bootstrap/app.php"))
    }

    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).unwrap_or(path).to_string_lossy().into_owned()
    }

    pub fn file_for(&self, fqcn: &str) -> Option<PathBuf> {
        file_in_roots(self.psr4.iter(), fqcn).or_else(|| self.vendor.file_for(fqcn))
    }

    pub fn root_for(&self, fqcn: &str) -> Option<&str> {
        longest_prefix(&self.psr4, fqcn).map(|root| root.prefix.as_str())
    }
}

impl VendorIndex {
    /// Without a vendor/ directory, as in CI before `composer install`, composer.lock still names every
    /// package and its autoload roots, so vendor classes keep their packages but not their files.
    fn load(vendor: &Path) -> Self {
        if let Ok(installed) = read_json(&vendor.join("composer/installed.json")) {
            let packages = installed["packages"].as_array().cloned().unwrap_or_default();

            return VendorIndex::from_packages(&packages, |package| {
                vendor.join("composer").join(package["install-path"].as_str()?).into()
            });
        }

        let Ok(lock) = read_json(&vendor.with_file_name("composer.lock")) else {
            return VendorIndex::default();
        };

        let packages: Vec<Value> = ["packages", "packages-dev"]
            .iter()
            .flat_map(|key| lock[*key].as_array().cloned().unwrap_or_default())
            .collect();

        VendorIndex::from_packages(&packages, |package| vendor.join(package["name"].as_str()?).into())
    }

    fn from_packages(packages: &[Value], install_path: impl Fn(&Value) -> Option<PathBuf>) -> Self {
        let packages = packages
            .iter()
            .filter_map(|package| {
                let name = package["name"].as_str()?.to_string();
                let install_path = install_path(package)?;
                let install_path = install_path.canonicalize().unwrap_or(install_path);
                let psr4 = psr4_roots(&package["autoload"]["psr-4"], &install_path);
                let providers = package["extra"]["laravel"]["providers"]
                    .as_array()
                    .map(|providers| providers.iter().filter_map(|provider| provider.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();

                Some(VendorPackage { name, psr4, providers })
            })
            .collect();

        VendorIndex { packages }
    }

    pub fn package_for(&self, fqcn: &str) -> Option<&str> {
        self.packages
            .iter()
            .filter_map(|package| longest_prefix(&package.psr4, fqcn).map(|root| (root.prefix.len(), package)))
            .max_by_key(|(length, _)| *length)
            .map(|(_, package)| package.name.as_str())
    }

    pub fn file_for(&self, fqcn: &str) -> Option<PathBuf> {
        file_in_roots(self.packages.iter().flat_map(|package| package.psr4.iter()), fqcn)
    }
}

fn psr4_roots(map: &Value, base: &Path) -> Vec<Psr4Root> {
    let Some(map) = map.as_object() else {
        return Vec::new();
    };

    map.iter()
        .map(|(prefix, dirs)| {
            let dirs = match dirs {
                Value::String(dir) => vec![dir.clone()],
                Value::Array(dirs) => dirs.iter().filter_map(|dir| dir.as_str().map(str::to_string)).collect(),
                _ => Vec::new(),
            };

            Psr4Root {
                prefix: prefix.trim_end_matches('\\').to_string(),
                dirs: dirs.iter().map(|dir| base.join(dir)).collect(),
            }
        })
        .collect()
}

fn file_in_roots<'a>(roots: impl Iterator<Item = &'a Psr4Root>, fqcn: &str) -> Option<PathBuf> {
    let mut candidates: Vec<&Psr4Root> = roots.filter(|root| matches_prefix(&root.prefix, fqcn)).collect();

    candidates.sort_by_key(|root| std::cmp::Reverse(root.prefix.len()));

    candidates.into_iter().find_map(|root| {
        let relative = fqcn[root.prefix.len()..].trim_start_matches('\\').replace('\\', "/") + ".php";

        root.dirs.iter().map(|dir| dir.join(&relative)).find(|path| path.is_file())
    })
}

fn longest_prefix<'a>(roots: &'a [Psr4Root], fqcn: &str) -> Option<&'a Psr4Root> {
    roots.iter().filter(|root| matches_prefix(&root.prefix, fqcn)).max_by_key(|root| root.prefix.len())
}

fn matches_prefix(prefix: &str, fqcn: &str) -> bool {
    prefix.is_empty()
        || (fqcn.len() > prefix.len()
            && fqcn.as_bytes()[prefix.len()] == b'\\'
            && fqcn[..prefix.len()].eq_ignore_ascii_case(prefix))
}

fn collect_php_files(dir: &Path, files: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            collect_php_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "php") {
            files.insert(path.canonicalize().unwrap_or(path));
        }
    }
}

/// A package installed under another project's vendor/ (laravel/framework inside an app) has no
/// vendor/ of its own; its dependencies are the enclosing vendor/ directory.
fn vendor_dir(root: &Path) -> PathBuf {
    root.ancestors()
        .skip(1)
        .find(|dir| dir.file_name().is_some_and(|name| name == "vendor") && dir.join("composer/installed.json").is_file())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join("vendor"))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let contents = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;

    serde_json::from_str(&contents).map_err(|error| format!("{}: {error}", path.display()))
}
