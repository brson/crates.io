use std::collections::HashMap;
use std::io::fs::PathExtensions;
use std::io::{self, fs, File, MemWriter};
use std::iter::repeat;
use std::sync::Arc;

use conduit::{Handler, Request, Method};
use conduit_test::MockRequest;
use git2;
use rustc_serialize::{json, Decoder};
use semver;

use cargo_registry::App;
use cargo_registry::dependency::EncodableDependency;
use cargo_registry::download::EncodableVersionDownload;
use cargo_registry::krate::{Crate, EncodableCrate};
use cargo_registry::upload as u;
use cargo_registry::user::EncodableUser;
use cargo_registry::version::EncodableVersion;

#[derive(RustcDecodable)]
struct CrateList { crates: Vec<EncodableCrate>, meta: CrateMeta }
#[derive(RustcDecodable)]
struct VersionsList { versions: Vec<EncodableVersion> }
#[derive(RustcDecodable)]
struct CrateMeta { total: int }
#[derive(RustcDecodable)]
struct GitCrate { name: String, vers: String, deps: Vec<String>, cksum: String }
#[derive(RustcDecodable)]
struct GoodCrate { krate: EncodableCrate }
#[derive(RustcDecodable)]
struct CrateResponse { krate: EncodableCrate, versions: Vec<EncodableVersion> }
#[derive(RustcDecodable)]
struct Deps { dependencies: Vec<EncodableDependency> }
#[derive(RustcDecodable)]
struct RevDeps { dependencies: Vec<EncodableDependency>, meta: CrateMeta }
#[derive(RustcDecodable)]
struct Downloads { version_downloads: Vec<EncodableVersionDownload> }

#[test]
fn index() {
    let (_b, _app, mut middle) = ::app();
    let mut req = MockRequest::new(Method::Get, "/api/v1/crates");
    let mut response = ok_resp!(middle.call(&mut req));
    let json: CrateList = ::json(&mut response);
    assert_eq!(json.crates.len(), 0);
    assert_eq!(json.meta.total, 0);

    let krate = ::krate("foo");
    middle.add(::middleware::MockUser(::user("foo")));
    middle.add(::middleware::MockCrate(krate.clone()));
    let mut response = ok_resp!(middle.call(&mut req));
    let json: CrateList = ::json(&mut response);
    assert_eq!(json.crates.len(), 1);
    assert_eq!(json.meta.total, 1);
    assert_eq!(json.crates[0].name, krate.name);
    assert_eq!(json.crates[0].id, krate.name);
}

#[test]
fn index_queries() {
    let (_b, app, middle) = ::app();

    let mut req = ::req(app, Method::Get, "/api/v1/crates");
    let u = ::mock_user(&mut req, ::user("foo"));
    let mut krate = ::krate("foo");
    krate.keywords.push("kw1".to_string());
    krate.readme = Some("readme".to_string());
    krate.description = Some("description".to_string());
    ::mock_crate(&mut req, krate);
    ::mock_crate(&mut req, ::krate("BAR"));

    let mut response = ok_resp!(middle.call(req.with_query("q=baz")));
    assert_eq!(::json::<CrateList>(&mut response).meta.total, 0);

    // All of these fields should be indexed/searched by the queries
    let mut response = ok_resp!(middle.call(req.with_query("q=foo")));
    assert_eq!(::json::<CrateList>(&mut response).meta.total, 1);
    let mut response = ok_resp!(middle.call(req.with_query("q=kw1")));
    assert_eq!(::json::<CrateList>(&mut response).meta.total, 1);
    let mut response = ok_resp!(middle.call(req.with_query("q=readme")));
    assert_eq!(::json::<CrateList>(&mut response).meta.total, 1);
    let mut response = ok_resp!(middle.call(req.with_query("q=description")));
    assert_eq!(::json::<CrateList>(&mut response).meta.total, 1);

    let query = format!("user_id={}", u.id);
    let mut response = ok_resp!(middle.call(req.with_query(query.as_slice())));
    assert_eq!(::json::<CrateList>(&mut response).crates.len(), 2);
    let mut response = ok_resp!(middle.call(req.with_query("user_id=0")));
    assert_eq!(::json::<CrateList>(&mut response).crates.len(), 0);

    let mut response = ok_resp!(middle.call(req.with_query("letter=F")));
    assert_eq!(::json::<CrateList>(&mut response).crates.len(), 1);
    let mut response = ok_resp!(middle.call(req.with_query("letter=B")));
    assert_eq!(::json::<CrateList>(&mut response).crates.len(), 1);
    let mut response = ok_resp!(middle.call(req.with_query("letter=b")));
    assert_eq!(::json::<CrateList>(&mut response).crates.len(), 1);
    let mut response = ok_resp!(middle.call(req.with_query("letter=c")));
    assert_eq!(::json::<CrateList>(&mut response).crates.len(), 0);

    let mut response = ok_resp!(middle.call(req.with_query("keyword=kw1")));
    assert_eq!(::json::<CrateList>(&mut response).crates.len(), 1);
    let mut response = ok_resp!(middle.call(req.with_query("keyword=kw2")));
    assert_eq!(::json::<CrateList>(&mut response).crates.len(), 0);
}

#[test]
fn show() {
    let (_b, _app, mut middle) = ::app();
    let mut krate = ::krate("foo");
    krate.description = Some(format!("description"));
    krate.documentation = Some(format!("https://example.com"));
    krate.homepage = Some(format!("http://example.com"));
    middle.add(::middleware::MockUser(::user("foo")));
    middle.add(::middleware::MockCrate(krate.clone()));
    let mut req = MockRequest::new(Method::Get,
                                   format!("/api/v1/crates/{}", krate.name).as_slice());
    let mut response = ok_resp!(middle.call(&mut req));
    let json: CrateResponse = ::json(&mut response);
    assert_eq!(json.krate.name, krate.name);
    assert_eq!(json.krate.id, krate.name);
    assert_eq!(json.krate.description, krate.description);
    assert_eq!(json.krate.homepage, krate.homepage);
    assert_eq!(json.krate.documentation, krate.documentation);
    let versions = json.krate.versions.as_ref().unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(json.versions.len(), 1);
    assert_eq!(json.versions[0].id, versions[0]);
    assert_eq!(json.versions[0].krate, json.krate.id);
    assert_eq!(json.versions[0].num, "1.0.0".to_string());
    let suffix = "/api/v1/crates/foo/1.0.0/download";
    assert!(json.versions[0].dl_path.as_slice().ends_with(suffix),
            "bad suffix {}", json.versions[0].dl_path);
}

#[test]
fn versions() {
    let (_b, app, middle) = ::app();
    let mut req = ::req(app, Method::Get, "/api/v1/crates/foo/versions");
    ::mock_user(&mut req, ::user("foo"));
    ::mock_crate(&mut req, ::krate("foo"));
    let mut response = ok_resp!(middle.call(&mut req));
    let json: VersionsList = ::json(&mut response);
    assert_eq!(json.versions.len(), 1);
}

fn new_req(app: Arc<App>, krate: &str, version: &str) -> MockRequest {
    new_req_full(app, ::krate(krate), version, Vec::new())
}

fn new_req_full(app: Arc<App>, krate: Crate, version: &str,
                deps: Vec<u::CrateDependency>) -> MockRequest {
    let mut req = ::req(app, Method::Put, "/api/v1/crates/new");
    req.with_body(new_req_body(krate, version, deps).as_slice());
    return req;
}

fn new_req_body(krate: Crate, version: &str, deps: Vec<u::CrateDependency>)
                -> Vec<u8> {
    let kws = krate.keywords.into_iter().map(u::Keyword).collect();
    new_crate_to_body(&u::NewCrate {
        name: u::CrateName(krate.name),
        vers: u::CrateVersion(semver::Version::parse(version).unwrap()),
        features: HashMap::new(),
        deps: deps,
        authors: vec!["foo".to_string()],
        description: Some("description".to_string()),
        homepage: krate.homepage,
        documentation: krate.documentation,
        readme: krate.readme,
        keywords: Some(u::KeywordList(kws)),
        license: Some("MIT".to_string()),
        license_file: None,
        repository: krate.repository,
    })
}

fn new_crate_to_body(new_crate: &u::NewCrate) -> Vec<u8> {
    let json = json::encode(&new_crate);
    let mut body = MemWriter::new();
    body.write_le_u32(json.len() as u32).unwrap();
    body.write_str(json.as_slice()).unwrap();
    body.write_le_u32(0).unwrap();
    body.into_inner()
}

#[test]
fn new_wrong_token() {
    let (_b, app, middle) = ::app();
    let mut req = new_req(app.clone(), "foo", "1.0.0");
    bad_resp!(middle.call(&mut req));
    drop(req);

    let mut req = new_req(app.clone(), "foo", "1.0.0");
    req.header("Authorization", "bad");
    bad_resp!(middle.call(&mut req));
    drop(req);

    let mut req = new_req(app, "foo", "1.0.0");
    ::mock_user(&mut req, ::user("foo"));
    ::logout(&mut req);
    req.header("Authorization", "bad");
    bad_resp!(middle.call(&mut req));
}

#[test]
fn new_bad_names() {
    fn bad_name(name: &str) {
        println!("testing: `{}`", name);
        let (_b, app, middle) = ::app();
        let mut req = new_req(app, name, "1.0.0");
        ::mock_user(&mut req, ::user("foo"));
        ::logout(&mut req);
        let json = bad_resp!(middle.call(&mut req));
        assert!(json.errors[0].detail.as_slice().contains("invalid crate name"),
                "{}", json.errors);
    }

    bad_name("");
    bad_name("foo bar");
}

#[test]
fn new_krate() {
    let (_b, app, middle) = ::app();
    let mut req = new_req(app, "foo", "1.0.0");
    let user = ::mock_user(&mut req, ::user("foo"));
    ::logout(&mut req);
    req.header("Authorization", user.api_token.as_slice());
    let mut response = ok_resp!(middle.call(&mut req));
    let json: GoodCrate = ::json(&mut response);
    assert_eq!(json.krate.name.as_slice(), "foo");
    assert_eq!(json.krate.max_version.as_slice(), "1.0.0");
}

#[test]
fn new_krate_weird_version() {
    let (_b, app, middle) = ::app();
    let mut req = new_req(app, "foo", "0.0.0-pre");
    let user = ::mock_user(&mut req, ::user("foo"));
    ::logout(&mut req);
    req.header("Authorization", user.api_token.as_slice());
    let mut response = ok_resp!(middle.call(&mut req));
    let json: GoodCrate = ::json(&mut response);
    assert_eq!(json.krate.name.as_slice(), "foo");
    assert_eq!(json.krate.max_version.as_slice(), "0.0.0-pre");
}

#[test]
fn new_krate_with_dependency() {
    let (_b, app, middle) = ::app();
    let dep = u::CrateDependency {
        name: u::CrateName("foo".to_string()),
        optional: false,
        default_features: true,
        features: Vec::new(),
        version_req: u::CrateVersionReq(semver::VersionReq::parse(">= 0").unwrap()),
        target: None,
        kind: None,
    };
    let mut req = new_req_full(app, ::krate("new"), "1.0.0", vec![dep]);
    ::mock_user(&mut req, ::user("foo"));
    ::mock_crate(&mut req, ::krate("foo"));
    let mut response = ok_resp!(middle.call(&mut req));
    ::json::<GoodCrate>(&mut response);
}

#[test]
fn new_krate_twice() {
    let (_b, app, middle) = ::app();
    let mut krate = ::krate("foo");
    krate.description = Some("description".to_string());
    let mut req = new_req_full(app, krate.clone(), "2.0.0", Vec::new());
    ::mock_user(&mut req, ::user("foo"));
    ::mock_crate(&mut req, ::krate("foo"));
    let mut response = ok_resp!(middle.call(&mut req));
    let json: GoodCrate = ::json(&mut response);
    assert_eq!(json.krate.name, krate.name);
    assert_eq!(json.krate.description, krate.description);
}

#[test]
fn new_krate_wrong_user() {
    let (_b, app, middle) = ::app();

    let mut req = new_req(app, "foo", "2.0.0");

    // Create the 'foo' crate with one user
    ::mock_user(&mut req, ::user("foo"));
    ::mock_crate(&mut req, ::krate("foo"));

    // But log in another
    ::mock_user(&mut req, ::user("bar"));

    let json = bad_resp!(middle.call(&mut req));
    assert!(json.errors[0].detail.as_slice().contains("another user"),
            "{}", json.errors);
}

#[test]
fn new_krate_bad_name() {
    let (_b, app, middle) = ::app();

    {
        let mut req = new_req(app.clone(), "snow☃", "2.0.0");
        ::mock_user(&mut req, ::user("foo"));
        let json = bad_resp!(middle.call(&mut req));
        assert!(json.errors[0].detail.as_slice().contains("invalid crate name"),
                "{}", json.errors);
    }
    {
        let mut req = new_req(app, "áccênts", "2.0.0");
        ::mock_user(&mut req, ::user("foo"));
        let json = bad_resp!(middle.call(&mut req));
        assert!(json.errors[0].detail.as_slice().contains("invalid crate name"),
                "{}", json.errors);
    }
}

#[test]
fn new_crate_owner() {
    #[derive(RustcDecodable)] struct O { ok: bool }

    let (_b, app, middle) = ::app();

    // Create a crate under one user
    let mut req = new_req(app.clone(), "foo", "1.0.0");
    let u2 = ::mock_user(&mut req, ::user("bar"));
    ::mock_user(&mut req, ::user("foo"));
    let mut response = ok_resp!(middle.call(&mut req));
    ::json::<GoodCrate>(&mut response);

    // Flag the second user as an owner
    let body = r#"{"users":["bar"]}"#;
    let mut response = ok_resp!(middle.call(req.with_path("/api/v1/crates/foo/owners")
                                               .with_method(Method::Put)
                                               .with_body(body)));
    assert!(::json::<O>(&mut response).ok);

    // And upload a new crate as the first user
    let body = new_req_body(::krate("foo"), "2.0.0", Vec::new());
    req.mut_extensions().insert(u2);
    let mut response = ok_resp!(middle.call(req.with_path("/api/v1/crates/new")
                                               .with_method(Method::Put)
                                               .with_body(body)));
    ::json::<GoodCrate>(&mut response);
}

#[test]
fn valid_feature_names() {
    assert!(Crate::valid_feature_name("foo"));
    assert!(!Crate::valid_feature_name(""));
    assert!(!Crate::valid_feature_name("/"));
    assert!(!Crate::valid_feature_name("%/%"));
    assert!(Crate::valid_feature_name("a/a"));
}

#[test]
fn new_krate_too_big() {
    let (_b, app, middle) = ::app();
    let mut req = new_req(app, "foo", "1.0.0");
    ::mock_user(&mut req, ::user("foo"));
    req.with_body(repeat("a").take(1000 * 1000).collect::<String>().as_slice());
    bad_resp!(middle.call(&mut req));
}

#[test]
fn new_krate_duplicate_version() {
    let (_b, app, middle) = ::app();
    let mut req = new_req(app, "foo", "1.0.0");
    ::mock_user(&mut req, ::user("foo"));
    ::mock_crate(&mut req, ::krate("foo"));
    let json = bad_resp!(middle.call(&mut req));
    assert!(json.errors[0].detail.as_slice().contains("already uploaded"),
            "{}", json.errors);
}

#[test]
fn new_crate_similar_name() {
    let (_b, app, middle) = ::app();
    let mut req = new_req(app, "foo", "1.0.0");
    ::mock_user(&mut req, ::user("foo"));
    ::mock_crate(&mut req, ::krate("Foo"));
    let json = bad_resp!(middle.call(&mut req));
    assert!(json.errors[0].detail.as_slice().contains("already uploaded"),
            "{}", json.errors);
}

#[test]
fn new_krate_git_upload() {
    let (_b, app, middle) = ::app();
    let mut req = new_req(app, "foo", "1.0.0");
    ::mock_user(&mut req, ::user("foo"));
    let mut response = ok_resp!(middle.call(&mut req));
    ::json::<GoodCrate>(&mut response);

    let path = ::git::checkout().join("3/f/foo");
    assert!(path.exists());
    let contents = File::open(&path).read_to_string().unwrap();
    let p: GitCrate = json::decode(contents.as_slice()).unwrap();
    assert_eq!(p.name.as_slice(), "foo");
    assert_eq!(p.vers.as_slice(), "1.0.0");
    assert!(p.deps.is_empty());
    assert_eq!(p.cksum.as_slice(),
               "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
}

#[test]
fn new_krate_git_upload_appends() {
    let (_b, app, middle) = ::app();
    let path = ::git::checkout().join("3/f/foo");
    fs::mkdir_recursive(&path.dir_path(), io::USER_RWX).unwrap();
    File::create(&path).write_str(
        r#"{"name":"foo","vers":"0.0.1","deps":[],"cksum":"3j3"}
"#
    ).unwrap();

    let mut req = new_req(app, "foo", "1.0.0");
    ::mock_user(&mut req, ::user("foo"));
    let mut response = ok_resp!(middle.call(&mut req));
    ::json::<GoodCrate>(&mut response);

    let contents = File::open(&path).read_to_string().unwrap();
    let mut lines = contents.as_slice().lines();
    let p1: GitCrate = json::decode(lines.next().unwrap().trim()).unwrap();
    let p2: GitCrate = json::decode(lines.next().unwrap().trim()).unwrap();
    assert!(lines.next().is_none());
    assert_eq!(p1.name.as_slice(), "foo");
    assert_eq!(p1.vers.as_slice(), "0.0.1");
    assert!(p1.deps.is_empty());
    assert_eq!(p2.name.as_slice(), "foo");
    assert_eq!(p2.vers.as_slice(), "1.0.0");
    assert!(p2.deps.is_empty());
}

#[test]
fn new_krate_git_upload_with_conflicts() {
    let (_b, app, middle) = ::app();

    {
        let repo = git2::Repository::open(&::git::bare()).unwrap();
        let target = repo.head().unwrap().target().unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.find_commit(target).unwrap();
        let tree = repo.find_tree(parent.tree_id()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "empty commit", &tree,
                    &[&parent]).unwrap();
    }

    let mut req = new_req(app, "foo", "1.0.0");
    ::mock_user(&mut req, ::user("foo"));
    let mut response = ok_resp!(middle.call(&mut req));
    ::json::<GoodCrate>(&mut response);
}

#[test]
fn new_krate_dependency_missing() {
    let (_b, app, middle) = ::app();
    let dep = u::CrateDependency {
        optional: false,
        default_features: true,
        name: u::CrateName("bar".to_string()),
        features: Vec::new(),
        version_req: u::CrateVersionReq(semver::VersionReq::parse(">= 0.0.0").unwrap()),
        target: None,
        kind: None,
    };
    let mut req = new_req_full(app, ::krate("foo"), "1.0.0", vec![dep]);
    ::mock_user(&mut req, ::user("foo"));
    let mut response = ok_resp!(middle.call(&mut req));
    let json = ::json::<::Bad>(&mut response);
    assert!(json.errors[0].detail.as_slice()
                .contains("no known crate named `bar`"));
}

#[test]
fn summary_doesnt_die() {
    let (_b, _app, middle) = ::app();
    let mut req = MockRequest::new(Method::Get, "/summary");
    ok_resp!(middle.call(&mut req));
}

#[test]
fn download() {
    let (_b, app, middle) = ::app();
    let mut req = ::req(app, Method::Get, "/api/v1/crates/foo/1.0.0/download");
    ::mock_user(&mut req, ::user("foo"));
    ::mock_crate(&mut req, ::krate("foo"));
    let resp = t_resp!(middle.call(&mut req));
    assert_eq!(resp.status.0, 302);

    req.with_path("/api/v1/crates/foo/1.0.0/downloads");
    let mut resp = ok_resp!(middle.call(&mut req));
    let downloads = ::json::<Downloads>(&mut resp);
    assert_eq!(downloads.version_downloads.len(), 1);

    req.with_path("/api/v1/crates/FOO/1.0.0/download");
    let resp = t_resp!(middle.call(&mut req));
    assert_eq!(resp.status.0, 302);

    req.with_path("/api/v1/crates/FOO/1.0.0/downloads");
    let mut resp = ok_resp!(middle.call(&mut req));
    let downloads = ::json::<Downloads>(&mut resp);
    assert_eq!(downloads.version_downloads.len(), 1);
}

#[test]
fn download_bad() {
    let (_b, _app, mut middle) = ::app();
    let user = ::user("foo");
    let krate = ::krate("foo");
    middle.add(::middleware::MockUser(user.clone()));
    middle.add(::middleware::MockCrate(krate.clone()));
    let rel = format!("/api/v1/crates/{}/0.1.0/download", krate.name);
    let mut req = MockRequest::new(Method::Get, rel.as_slice());
    let mut response = ok_resp!(middle.call(&mut req));
    ::json::<::Bad>(&mut response);
}

#[test]
fn dependencies() {
    let (_b, _app, mut middle) = ::app();
    let user = ::user("foo");
    let c1 = ::krate("foo");
    let c2 = ::krate("bar");
    middle.add(::middleware::MockUser(user.clone()));
    middle.add(::middleware::MockDependency(c1.clone(), "1.0.0", c2.clone()));
    let rel = format!("/api/v1/crates/{}/1.0.0/dependencies", c1.name);
    let mut req = MockRequest::new(Method::Get, rel.as_slice());
    let mut response = ok_resp!(middle.call(&mut req));
    let deps = ::json::<Deps>(&mut response);
    assert_eq!(deps.dependencies[0].crate_id.as_slice(), "bar");
    drop(req);

    let rel = format!("/api/v1/crates/{}/1.0.2/dependencies", c1.name);
    let mut req = MockRequest::new(Method::Get, rel.as_slice());
    let mut response = ok_resp!(middle.call(&mut req));
    ::json::<::Bad>(&mut response);
}

#[test]
fn following() {
    #[derive(RustcDecodable)] struct F { following: bool }
    #[derive(RustcDecodable)] struct O { ok: bool }

    let (_b, app, middle) = ::app();
    let mut req = ::req(app, Method::Get, "/api/v1/crates/foo/following");
    ::mock_user(&mut req, ::user("foo"));
    ::mock_crate(&mut req, ::krate("foo"));

    let mut response = ok_resp!(middle.call(&mut req));
    assert!(!::json::<F>(&mut response).following);

    req.with_path("/api/v1/crates/foo/follow")
       .with_method(Method::Put);
    let mut response = ok_resp!(middle.call(&mut req));
    assert!(::json::<O>(&mut response).ok);
    let mut response = ok_resp!(middle.call(&mut req));
    assert!(::json::<O>(&mut response).ok);

    req.with_path("/api/v1/crates/foo/following")
       .with_method(Method::Get);
    let mut response = ok_resp!(middle.call(&mut req));
    assert!(::json::<F>(&mut response).following);

    req.with_path("/api/v1/crates")
       .with_query("following=1");
    let mut response = ok_resp!(middle.call(&mut req));
    let l = ::json::<CrateList>(&mut response);
    assert_eq!(l.crates.len(), 1);

    req.with_path("/api/v1/crates/foo/follow")
       .with_method(Method::Delete);
    let mut response = ok_resp!(middle.call(&mut req));
    assert!(::json::<O>(&mut response).ok);
    let mut response = ok_resp!(middle.call(&mut req));
    assert!(::json::<O>(&mut response).ok);

    req.with_path("/api/v1/crates/foo/following")
       .with_method(Method::Get);
    let mut response = ok_resp!(middle.call(&mut req));
    assert!(!::json::<F>(&mut response).following);

    req.with_path("/api/v1/crates")
       .with_query("following=1")
       .with_method(Method::Get);
    let mut response = ok_resp!(middle.call(&mut req));
    assert_eq!(::json::<CrateList>(&mut response).crates.len(), 0);
}

#[test]
fn owners() {
    #[derive(RustcDecodable)] struct R { users: Vec<EncodableUser> }
    #[derive(RustcDecodable)] struct O { ok: bool }

    let (_b, app, middle) = ::app();
    let mut req = ::req(app, Method::Get, "/api/v1/crates/foo/owners");
    let other = ::user("foobar");
    ::mock_user(&mut req, other);
    ::mock_user(&mut req, ::user("foo"));
    ::mock_crate(&mut req, ::krate("foo"));

    let mut response = ok_resp!(middle.call(&mut req));
    let r: R = ::json(&mut response);
    assert_eq!(r.users.len(), 1);

    let mut response = ok_resp!(middle.call(req.with_method(Method::Get)));
    let r: R = ::json(&mut response);
    assert_eq!(r.users.len(), 1);

    let body = r#"{"users":["foobar"]}"#;
    let mut response = ok_resp!(middle.call(req.with_method(Method::Put)
                                               .with_body(body)));
    assert!(::json::<O>(&mut response).ok);

    let mut response = ok_resp!(middle.call(req.with_method(Method::Get)));
    let r: R = ::json(&mut response);
    assert_eq!(r.users.len(), 2);

    let body = r#"{"users":["foobar"]}"#;
    let mut response = ok_resp!(middle.call(req.with_method(Method::Delete)
                                               .with_body(body)));
    assert!(::json::<O>(&mut response).ok);

    let mut response = ok_resp!(middle.call(req.with_method(Method::Get)));
    let r: R = ::json(&mut response);
    assert_eq!(r.users.len(), 1);

    let body = r#"{"users":["foo"]}"#;
    let mut response = ok_resp!(middle.call(req.with_method(Method::Delete)
                                               .with_body(body)));
    ::json::<::Bad>(&mut response);
}

#[test]
fn yank() {
    #[derive(RustcDecodable)] struct O { ok: bool }
    #[derive(RustcDecodable)] struct V { version: EncodableVersion }
    let (_b, app, middle) = ::app();
    let path = ::git::checkout().join("3/f/foo");

    // Upload a new crate, putting it in the git index
    let mut req = new_req(app, "foo", "1.0.0");
    ::mock_user(&mut req, ::user("foo"));
    let mut response = ok_resp!(middle.call(&mut req));
    ::json::<GoodCrate>(&mut response);
    assert!(File::open(&path).read_to_string().unwrap().as_slice()
                             .contains("\"yanked\":false"));

    // make sure it's not yanked
    let mut r = ok_resp!(middle.call(req.with_method(Method::Get)
                                        .with_path("/api/v1/crates/foo/1.0.0")));
    assert!(!::json::<V>(&mut r).version.yanked);

    // yank it
    let mut r = ok_resp!(middle.call(req.with_method(Method::Delete)
                                        .with_path("/api/v1/crates/foo/1.0.0/yank")));
    assert!(::json::<O>(&mut r).ok);
    assert!(File::open(&path).read_to_string().unwrap().as_slice()
                             .contains("\"yanked\":true"));
    let mut r = ok_resp!(middle.call(req.with_method(Method::Get)
                                        .with_path("/api/v1/crates/foo/1.0.0")));
    assert!(::json::<V>(&mut r).version.yanked);

    // un-yank it
    let mut r = ok_resp!(middle.call(req.with_method(Method::Put)
                                        .with_path("/api/v1/crates/foo/1.0.0/unyank")));
    assert!(::json::<O>(&mut r).ok);
    assert!(File::open(&path).read_to_string().unwrap().as_slice()
                             .contains("\"yanked\":false"));
    let mut r = ok_resp!(middle.call(req.with_method(Method::Get)
                                        .with_path("/api/v1/crates/foo/1.0.0")));
    assert!(!::json::<V>(&mut r).version.yanked);
}

#[test]
fn yank_not_owner() {
    let (_b, app, middle) = ::app();
    let mut req = ::req(app, Method::Delete, "/api/v1/crates/foo/1.0.0/yank");
    ::mock_user(&mut req, ::user("foo"));
    ::mock_crate(&mut req, ::krate("foo"));
    ::mock_user(&mut req, ::user("bar"));
    let mut response = ok_resp!(middle.call(&mut req));
    ::json::<::Bad>(&mut response);
}

#[test]
fn bad_keywords() {
    let (_b, app, middle) = ::app();
    {
        let mut krate = ::krate("foo");
        krate.keywords.push("super-long-keyword-name-oh-no".to_string());
        let mut req = new_req_full(app.clone(), krate, "1.0.0", Vec::new());
        ::mock_user(&mut req, ::user("foo"));
        let mut response = ok_resp!(middle.call(&mut req));
        ::json::<::Bad>(&mut response);
    }
    {
        let mut krate = ::krate("foo");
        krate.keywords.push("?@?%".to_string());
        let mut req = new_req_full(app.clone(), krate, "1.0.0", Vec::new());
        ::mock_user(&mut req, ::user("foo"));
        let mut response = ok_resp!(middle.call(&mut req));
        ::json::<::Bad>(&mut response);
    }
    {
        let mut krate = ::krate("foo");
        krate.keywords.push("?@?%".to_string());
        let mut req = new_req_full(app.clone(), krate, "1.0.0", Vec::new());
        ::mock_user(&mut req, ::user("foo"));
        let mut response = ok_resp!(middle.call(&mut req));
        ::json::<::Bad>(&mut response);
    }
    {
        let mut krate = ::krate("foo");
        krate.keywords.push("áccênts".to_string());
        let mut req = new_req_full(app.clone(), krate, "1.0.0", Vec::new());
        ::mock_user(&mut req, ::user("foo"));
        let mut response = ok_resp!(middle.call(&mut req));
        ::json::<::Bad>(&mut response);
    }
}

#[test]
fn reverse_dependencies() {
    let (_b, _app, mut middle) = ::app();
    let user = ::user("foo");
    let c1 = ::krate("foo");
    let c2 = ::krate("bar");
    middle.add(::middleware::MockUser(user.clone()));
    // multiple dependencies of c1 on c2, to detect we handle this
    // correctly.
    middle.add(::middleware::MockDependency(c1.clone(), "1.0.0", c2.clone()));
    middle.add(::middleware::MockDependency(c1.clone(), "1.1.0", c2.clone()));

    let rel = format!("/api/v1/crates/{}/reverse_dependencies", c2.name);
    let mut req = MockRequest::new(Method::Get, rel.as_slice());
    let mut response = ok_resp!(middle.call(&mut req));
    let deps = ::json::<RevDeps>(&mut response);
    assert_eq!(deps.dependencies.len(), 1);
    assert_eq!(deps.meta.total, 1);
    assert_eq!(deps.dependencies[0].crate_id.as_slice(), &*c1.name);
    drop(req);

    // c1 has no dependent crates.
    let rel = format!("/api/v1/crates/{}/reverse_dependencies", c1.name);
    let mut req = MockRequest::new(Method::Get, rel.as_slice());
    let mut response = ok_resp!(middle.call(&mut req));
    let deps = ::json::<RevDeps>(&mut response);
    assert_eq!(deps.dependencies.len(), 0);
    assert_eq!(deps.meta.total, 0);
    drop(req);
}

#[test]
fn author_license_and_description_required() {
    let (_b, app, middle) = ::app();
    ::user("foo");

    let mut req = ::req(app, Method::Put, "/api/v1/crates/new");
    let mut new_crate = u::NewCrate {
        name: u::CrateName("foo".to_string()),
        vers: u::CrateVersion(semver::Version::parse("1.0.0").unwrap()),
        features: HashMap::new(),
        deps: Vec::new(),
        authors: Vec::new(),
        description: None,
        homepage: None,
        documentation: None,
        readme: None,
        keywords: None,
        license: None,
        license_file: None,
        repository: None,
    };
    req.with_body(new_crate_to_body(&new_crate));
    let json = bad_resp!(middle.call(&mut req));
    assert!(json.errors[0].detail.as_slice().contains("author") &&
            json.errors[0].detail.as_slice().contains("description") &&
            json.errors[0].detail.as_slice().contains("license"),
            "{}", json.errors);

    new_crate.license = Some("MIT".to_string());
    new_crate.authors.push("".to_string());
    req.with_body(new_crate_to_body(&new_crate));
    let json = bad_resp!(middle.call(&mut req));
    assert!(json.errors[0].detail.as_slice().contains("author") &&
            json.errors[0].detail.as_slice().contains("description") &&
            !json.errors[0].detail.as_slice().contains("license"),
            "{}", json.errors);

    new_crate.license = None;
    new_crate.license_file = Some("foo".to_string());
    new_crate.authors.push("foo".to_string());
    req.with_body(new_crate_to_body(&new_crate));
    let json = bad_resp!(middle.call(&mut req));
    assert!(!json.errors[0].detail.as_slice().contains("author") &&
            json.errors[0].detail.as_slice().contains("description") &&
            !json.errors[0].detail.as_slice().contains("license"),
            "{}", json.errors);
}
