//! S11: identifiers are validated at construction.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{
    CommitId, ContentHash, DraftId, Guid, IdempotencyKey, LineRange, LogicalFile, RequestId, SettingPath,
    SwimlaneId, TenantId, UserId,
};

#[test]
fn tenant_id_rejects_invalid_chars() {
    for ok in ["sit1", "sit-2", "t-sit1", "a", "uat_3"] {
        assert!(TenantId::parse(ok).is_ok(), "{ok:?}");
    }
    let too_long = "a".repeat(64);
    for bad in [
        "",
        "-sit1",
        "_x",
        "Sit1",
        "sit 1",
        "sit.1",
        "sit/1",
        "sit\\1",
        "sit\n",
        "sit\0",
        "templates/x",
        "é",
        &too_long,
    ] {
        assert!(TenantId::parse(bad).is_err(), "{bad:?} must be rejected");
    }
}

#[test]
fn swimlane_id_is_a_slug() {
    for ok in ["sitb", "sit-2", "a", &"a".repeat(63)] {
        assert!(SwimlaneId::parse(ok).is_ok(), "{ok:?}");
    }
    for bad in ["", "-a", "A", "a_b", "a b", "a/b", &"a".repeat(64)] {
        assert!(SwimlaneId::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn content_hash_is_64_hex_and_round_trips() {
    let hex = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    let h: ContentHash = hex.parse().unwrap();
    assert_eq!(h.to_string(), hex);
    assert_eq!(h.as_bytes()[0], 0x00);
    assert_eq!(h.as_bytes()[31], 0xff);
    assert_eq!(serde_json::to_string(&h).unwrap(), format!("\"{hex}\""));
    assert_eq!(
        serde_json::from_str::<ContentHash>(&format!("\"{hex}\"")).unwrap(),
        h
    );
    // upper case is accepted and normalised
    let upper: ContentHash = hex.to_uppercase().parse().unwrap();
    assert_eq!(upper, h);
    for bad in ["", "abc", &hex[..63], &format!("{hex}0"), &"g".repeat(64)] {
        assert!(bad.parse::<ContentHash>().is_err(), "{bad:?}");
    }
    assert_eq!(ContentHash::from_bytes([7; 32]).as_bytes(), &[7; 32]);
}

#[test]
fn commit_id_is_40_or_64_hex() {
    assert!(CommitId::parse(&"a".repeat(40)).is_ok());
    assert!(CommitId::parse(&"A".repeat(40)).is_ok());
    assert_eq!(CommitId::parse(&"A".repeat(40)).unwrap().as_str(), "a".repeat(40));
    assert!(CommitId::parse(&"a".repeat(64)).is_ok());
    for bad in [
        "",
        &"a".repeat(39),
        &"a".repeat(41),
        &"a".repeat(63),
        &"z".repeat(40),
    ] {
        assert!(CommitId::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn guid_and_user_id() {
    let tid = "72F988BF-86F1-41AF-91AB-2D7CD011DB47";
    let oid = "0b6c3e0e-5a1d-4a52-8f55-3f9d6f0a7f11";
    let g = Guid::parse(tid).unwrap();
    assert_eq!(g.as_str(), tid.to_lowercase());
    let u = UserId::new(Guid::parse(tid).unwrap(), Guid::parse(oid).unwrap());
    assert_eq!(u.to_string(), format!("{}:{oid}", tid.to_lowercase()));
    assert_eq!(u.to_string().parse::<UserId>().unwrap(), u);
    for bad in [
        "",
        "not-a-guid",
        "72f988bf86f141af91ab2d7cd011db47",
        &format!("{tid}0"),
        "72f988bf-86f1-41af-91ab-2d7cd011db4g",
    ] {
        assert!(Guid::parse(bad).is_err(), "{bad:?}");
    }
    assert!("only-one-part".parse::<UserId>().is_err());
    let json = serde_json::to_string(&u).unwrap();
    assert_eq!(serde_json::from_str::<UserId>(&json).unwrap(), u);
}

#[test]
fn opaque_tokens_are_bounded_and_charset_limited() {
    for ok in ["abc", "A-b_c.9", &"x".repeat(128)] {
        assert!(RequestId::parse(ok).is_ok());
        assert!(IdempotencyKey::parse(ok).is_ok());
        assert!(DraftId::parse(ok).is_ok());
    }
    for bad in ["", "a b", "a/b", "a\n", &"x".repeat(129), "é"] {
        assert!(RequestId::parse(bad).is_err(), "{bad:?}");
        assert!(IdempotencyKey::parse(bad).is_err(), "{bad:?}");
        assert!(DraftId::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn setting_path_is_bounded_and_printable() {
    for ok in [
        "a.b.c",
        "a.b[0]",
        "a[\"x.y\"].z",
        "#2/a.b",
        "txInfinityOptions.remote-itm-teller.enableAccountSorting",
    ] {
        assert!(SettingPath::parse(ok).is_ok(), "{ok:?}");
    }
    let long = "a".repeat(1025);
    for bad in ["", "a\0b", "a\nb", &long] {
        assert!(SettingPath::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn line_range_is_one_based_and_ordered() {
    assert!(LineRange::new(1, 1).is_ok());
    assert!(LineRange::new(3, 10).is_ok());
    assert!(LineRange::new(0, 1).is_err());
    assert!(LineRange::new(5, 4).is_err());
    let r = LineRange::new(2, 4).unwrap();
    assert_eq!((r.start(), r.end()), (2, 4));
    assert!(serde_json::from_str::<LineRange>("{\"start\":5,\"end\":4}").is_err());
    assert_eq!(serde_json::to_string(&r).unwrap(), "{\"start\":2,\"end\":4}");
}

#[test]
fn logical_file_wraps_a_non_root_nfs_path() {
    assert!(LogicalFile::parse("a/b.yml").is_ok());
    assert!(LogicalFile::parse("../b.yml").is_err());
    assert!(LogicalFile::parse("").is_err());
}
