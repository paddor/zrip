// Property-based tests. Proptest calls getcwd which Miri's isolation blocks.
#![cfg(all(feature = "std", not(miri)))]

mod proptest_roundtrip {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn roundtrip_random_data(data in proptest::collection::vec(any::<u8>(), 0..50_000)) {
            let level = 1;
            let compressed = zrip::compress(&data, level).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
        }

        #[test]
        fn roundtrip_random_data_all_levels(
            data in proptest::collection::vec(any::<u8>(), 100..10_000),
            level in prop_oneof![Just(-7i32), Just(-5), Just(-3), Just(-1), Just(1), Just(2), Just(3), Just(4)]
        ) {
            let compressed = zrip::compress(&data, level).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
        }

        #[test]
        fn decompress_corrupt_never_panics(data in proptest::collection::vec(any::<u8>(), 4..200)) {
            let _ = zrip::decompress(&data);
        }

        #[test]
        fn roundtrip_repetitive_varying_period(
            period in 1usize..256,
            count in 1usize..1000,
        ) {
            let original: Vec<u8> = (0..period).map(|i| i as u8).collect::<Vec<_>>()
                .into_iter().cycle().take(period * count).collect();
            let compressed = zrip::compress(&original, 1).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &original);
        }
    }
}

#[cfg(not(miri))]
mod proptest_c_interop {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn roundtrip_random_data_c_cross_validate(data in proptest::collection::vec(any::<u8>(), 0..50_000)) {
            let compressed = zrip::compress(&data, 1).unwrap();
            let c_decompressed = zstd::decode_all(&compressed[..]).unwrap();
            prop_assert_eq!(&c_decompressed, &data);
        }

        #[test]
        fn roundtrip_random_data_all_levels_c_cross_validate(
            data in proptest::collection::vec(any::<u8>(), 100..10_000),
            level in prop_oneof![Just(-7i32), Just(-5), Just(-3), Just(-1), Just(1), Just(2), Just(3), Just(4)]
        ) {
            let compressed = zrip::compress(&data, level).unwrap();
            let c_decompressed = zstd::decode_all(&compressed[..]).unwrap();
            prop_assert_eq!(&c_decompressed, &data);
        }

        #[test]
        fn decompress_c_random_data(data in proptest::collection::vec(any::<u8>(), 0..50_000)) {
            let compressed = zstd::encode_all(&data[..], 1).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
        }

        #[test]
        fn decompress_c_random_levels(
            data in proptest::collection::vec(any::<u8>(), 100..10_000),
            level in 1..=22i32,
        ) {
            let compressed = zstd::encode_all(&data[..], level).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
        }

        #[test]
        fn roundtrip_repetitive_varying_period_c_cross_validate(
            period in 1usize..256,
            count in 1usize..1000,
        ) {
            let original: Vec<u8> = (0..period).map(|i| i as u8).collect::<Vec<_>>()
                .into_iter().cycle().take(period * count).collect();
            let compressed = zrip::compress(&original, 1).unwrap();
            let c_dec = zstd::decode_all(&compressed[..]).unwrap();
            prop_assert_eq!(&c_dec, &original);
        }
    }
}

#[cfg(not(miri))]
mod proptest_adversarial {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        #[test]
        fn corrupt_valid_frame_never_panics(
            data in proptest::collection::vec(any::<u8>(), 100..5000),
            corrupt_positions in proptest::collection::vec(0usize..1000, 1..10),
            corrupt_values in proptest::collection::vec(any::<u8>(), 1..10),
        ) {
            let compressed = zstd::encode_all(&data[..], 1).unwrap();
            let mut corrupted = compressed.clone();
            for (pos_raw, val) in corrupt_positions.iter().zip(corrupt_values.iter()) {
                let pos = pos_raw % corrupted.len();
                corrupted[pos] = *val;
            }
            let _ = zrip::decompress(&corrupted);
        }

        #[test]
        fn truncated_valid_frame_never_panics(
            data in proptest::collection::vec(any::<u8>(), 50..5000),
            truncate_frac in 0.0f64..1.0,
        ) {
            let compressed = zstd::encode_all(&data[..], 1).unwrap();
            let truncate_at = (compressed.len() as f64 * truncate_frac) as usize;
            let _ = zrip::decompress(&compressed[..truncate_at]);
        }

        #[test]
        fn spliced_frames_never_panic(
            data_a in proptest::collection::vec(any::<u8>(), 100..2000),
            data_b in proptest::collection::vec(any::<u8>(), 100..2000),
            splice_point_frac in 0.1f64..0.9,
        ) {
            let comp_a = zstd::encode_all(&data_a[..], 1).unwrap();
            let comp_b = zstd::encode_all(&data_b[..], 1).unwrap();
            let splice_a = (comp_a.len() as f64 * splice_point_frac) as usize;
            let splice_b = (comp_b.len() as f64 * splice_point_frac) as usize;
            let mut spliced = comp_a[..splice_a].to_vec();
            spliced.extend_from_slice(&comp_b[splice_b..]);
            let _ = zrip::decompress(&spliced);
        }

        #[test]
        fn arbitrary_garbage_with_valid_magic_never_panics(
            garbage in proptest::collection::vec(any::<u8>(), 0..500),
        ) {
            let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD];
            frame.extend_from_slice(&garbage);
            let _ = zrip::decompress(&frame);
        }
    }
}

mod proptest_adversarial_zrip {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        #[test]
        fn corrupt_zrip_frame_never_panics(
            data in proptest::collection::vec(any::<u8>(), 100..5000),
            level in prop_oneof![Just(-1i32), Just(1), Just(3)],
            corrupt_positions in proptest::collection::vec(0usize..1000, 1..10),
            corrupt_values in proptest::collection::vec(any::<u8>(), 1..10),
        ) {
            if let Ok(compressed) = zrip::compress(&data, level) {
                let mut corrupted = compressed.clone();
                for (pos_raw, val) in corrupt_positions.iter().zip(corrupt_values.iter()) {
                    let pos = pos_raw % corrupted.len();
                    corrupted[pos] = *val;
                }
                let _ = zrip::decompress(&corrupted);
            }
        }
    }
}

mod proptest_structured {
    use proptest::prelude::*;

    fn structured_data() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            (any::<u8>(), 1..50_000usize).prop_map(|(b, n)| vec![b; n]),
            (1usize..256, 100..10_000usize)
                .prop_map(|(period, len)| (0..len).map(|i| (i % period) as u8).collect()),
            (any::<[u8; 8]>(), any::<[u8; 8]>(), 100..5000usize).prop_map(|(a, b, n)| {
                let mut v = Vec::with_capacity(n);
                for i in 0..n {
                    if i % 16 < 8 {
                        v.push(a[i % 8]);
                    } else {
                        v.push(b[i % 8]);
                    }
                }
                v
            }),
            (any::<u8>(), any::<u8>(), 1000..20_000usize).prop_map(|(common, rare, n)| {
                (0..n)
                    .map(|i| if i % 100 == 0 { rare } else { common })
                    .collect()
            }),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn roundtrip_structured(
            data in structured_data(),
            level in prop_oneof![Just(-7i32), Just(-1), Just(1), Just(3), Just(4)]
        ) {
            let compressed = zrip::compress(&data, level).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
        }
    }

    #[cfg(not(miri))]
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn roundtrip_structured_c_cross_validate(
            data in structured_data(),
            level in prop_oneof![Just(-7i32), Just(-1), Just(1), Just(3), Just(4)]
        ) {
            let compressed = zrip::compress(&data, level).unwrap();
            let c_dec = zstd::decode_all(&compressed[..]).unwrap();
            prop_assert_eq!(&c_dec, &data);
        }

        #[test]
        fn decompress_c_structured(
            data in structured_data(),
            level in 1..=22i32,
        ) {
            let compressed = zstd::encode_all(&data[..], level).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
        }
    }
}
