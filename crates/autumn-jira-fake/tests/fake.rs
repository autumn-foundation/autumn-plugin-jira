//! Behaviour tests of `FakeJira` (AC22).

use autumn_jira_fake::sign;

#[test]
fn own_hmac_matches_the_vector() {
    // The Atlassian webhook page vector.
    assert_eq!(
        sign(b"It's a Secret to Everybody", b"Hello World!"),
        "sha256=a4771c39fbe90f317c7824e83ddef3caae9cb3d976c214ace1f2937e133263c9"
    );
    // RFC 4231 test case 1: a short key.
    assert_eq!(
        sign(&[0x0b; 20], b"Hi There"),
        "sha256=b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    // RFC 4231 test case 6: a key longer than one block.
    assert_eq!(
        sign(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        ),
        "sha256=60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
    // An empty key and an empty body.
    assert_eq!(
        sign(b"", b""),
        "sha256=b613679a0814d9ec772f95d778c35fc5ff1697c493715653c6c712144292c5ad"
    );
}
