use phoebe::core::ArrayValue;

#[test]
fn large_interpreter_arrays_retain_then_copy_on_write() {
    let words = (0..32).collect::<Vec<_>>();
    let original = ArrayValue::from_slice(&words).unwrap();
    let mut copy = original.clone();
    let (ArrayValue::Shared(original_storage), ArrayValue::Shared(copy_storage)) =
        (&original, &copy)
    else {
        panic!("large arrays should use managed storage")
    };
    assert!(original_storage.shares_storage_with(copy_storage));
    assert_eq!(original_storage.owners(), 2);

    copy.set(0, 99).unwrap();
    assert_eq!(original.as_slice(), words);
    assert_eq!(copy.as_slice()[0], 99);
    let (ArrayValue::Shared(original_storage), ArrayValue::Shared(copy_storage)) =
        (&original, &copy)
    else {
        unreachable!()
    };
    assert!(!original_storage.shares_storage_with(copy_storage));
    assert_eq!(original_storage.owners(), 1);
    assert_eq!(copy_storage.owners(), 1);

    copy.set(1, 7).unwrap();
    assert_eq!(copy.as_slice()[1], 7);
}

#[test]
fn small_interpreter_arrays_remain_inline() {
    let original = ArrayValue::from_slice(&[40, 2]).unwrap();
    let mut copy = original.clone();
    assert!(matches!(original, ArrayValue::Inline(_)));
    copy.set(0, 0).unwrap();
    assert_eq!(original.as_slice(), &[40, 2]);
    assert_eq!(copy.as_slice(), &[0, 2]);
}
