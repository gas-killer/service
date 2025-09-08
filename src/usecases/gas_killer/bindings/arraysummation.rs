///Module containing a contract's types and functions.
/**

```solidity
library BN254 {
    struct G1Point { uint256 X; uint256 Y; }
    struct G2Point { uint256[2] X; uint256[2] Y; }
}
```*/
#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets
)]
pub mod BN254 {
    use super::*;
    use alloy::sol_types as alloy_sol_types;
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**```solidity
struct G1Point { uint256 X; uint256 Y; }
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct G1Point {
        #[allow(missing_docs)]
        pub X: alloy::sol_types::private::primitives::aliases::U256,
        #[allow(missing_docs)]
        pub Y: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = (
            alloy::sol_types::sol_data::Uint<256>,
            alloy::sol_types::sol_data::Uint<256>,
        );
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = (
            alloy::sol_types::private::primitives::aliases::U256,
            alloy::sol_types::private::primitives::aliases::U256,
        );
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<G1Point> for UnderlyingRustTuple<'_> {
            fn from(value: G1Point) -> Self {
                (value.X, value.Y)
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for G1Point {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self { X: tuple.0, Y: tuple.1 }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolValue for G1Point {
            type SolType = Self;
        }
        #[automatically_derived]
        impl alloy_sol_types::private::SolTypeValue<Self> for G1Point {
            #[inline]
            fn stv_to_tokens(&self) -> <Self as alloy_sol_types::SolType>::Token<'_> {
                (
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self.X),
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self.Y),
                )
            }
            #[inline]
            fn stv_abi_encoded_size(&self) -> usize {
                if let Some(size) = <Self as alloy_sol_types::SolType>::ENCODED_SIZE {
                    return size;
                }
                let tuple = <UnderlyingRustTuple<
                    '_,
                > as ::core::convert::From<Self>>::from(self.clone());
                <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_encoded_size(&tuple)
            }
            #[inline]
            fn stv_eip712_data_word(&self) -> alloy_sol_types::Word {
                <Self as alloy_sol_types::SolStruct>::eip712_hash_struct(self)
            }
            #[inline]
            fn stv_abi_encode_packed_to(
                &self,
                out: &mut alloy_sol_types::private::Vec<u8>,
            ) {
                let tuple = <UnderlyingRustTuple<
                    '_,
                > as ::core::convert::From<Self>>::from(self.clone());
                <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_encode_packed_to(&tuple, out)
            }
            #[inline]
            fn stv_abi_packed_encoded_size(&self) -> usize {
                if let Some(size) = <Self as alloy_sol_types::SolType>::PACKED_ENCODED_SIZE {
                    return size;
                }
                let tuple = <UnderlyingRustTuple<
                    '_,
                > as ::core::convert::From<Self>>::from(self.clone());
                <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_packed_encoded_size(&tuple)
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolType for G1Point {
            type RustType = Self;
            type Token<'a> = <UnderlyingSolTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SOL_NAME: &'static str = <Self as alloy_sol_types::SolStruct>::NAME;
            const ENCODED_SIZE: Option<usize> = <UnderlyingSolTuple<
                '_,
            > as alloy_sol_types::SolType>::ENCODED_SIZE;
            const PACKED_ENCODED_SIZE: Option<usize> = <UnderlyingSolTuple<
                '_,
            > as alloy_sol_types::SolType>::PACKED_ENCODED_SIZE;
            #[inline]
            fn valid_token(token: &Self::Token<'_>) -> bool {
                <UnderlyingSolTuple<'_> as alloy_sol_types::SolType>::valid_token(token)
            }
            #[inline]
            fn detokenize(token: Self::Token<'_>) -> Self::RustType {
                let tuple = <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::detokenize(token);
                <Self as ::core::convert::From<UnderlyingRustTuple<'_>>>::from(tuple)
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolStruct for G1Point {
            const NAME: &'static str = "G1Point";
            #[inline]
            fn eip712_root_type() -> alloy_sol_types::private::Cow<'static, str> {
                alloy_sol_types::private::Cow::Borrowed("G1Point(uint256 X,uint256 Y)")
            }
            #[inline]
            fn eip712_components() -> alloy_sol_types::private::Vec<
                alloy_sol_types::private::Cow<'static, str>,
            > {
                alloy_sol_types::private::Vec::new()
            }
            #[inline]
            fn eip712_encode_type() -> alloy_sol_types::private::Cow<'static, str> {
                <Self as alloy_sol_types::SolStruct>::eip712_root_type()
            }
            #[inline]
            fn eip712_encode_data(&self) -> alloy_sol_types::private::Vec<u8> {
                [
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::eip712_data_word(&self.X)
                        .0,
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::eip712_data_word(&self.Y)
                        .0,
                ]
                    .concat()
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::EventTopic for G1Point {
            #[inline]
            fn topic_preimage_length(rust: &Self::RustType) -> usize {
                0usize
                    + <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::EventTopic>::topic_preimage_length(&rust.X)
                    + <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::EventTopic>::topic_preimage_length(&rust.Y)
            }
            #[inline]
            fn encode_topic_preimage(
                rust: &Self::RustType,
                out: &mut alloy_sol_types::private::Vec<u8>,
            ) {
                out.reserve(
                    <Self as alloy_sol_types::EventTopic>::topic_preimage_length(rust),
                );
                <alloy::sol_types::sol_data::Uint<
                    256,
                > as alloy_sol_types::EventTopic>::encode_topic_preimage(&rust.X, out);
                <alloy::sol_types::sol_data::Uint<
                    256,
                > as alloy_sol_types::EventTopic>::encode_topic_preimage(&rust.Y, out);
            }
            #[inline]
            fn encode_topic(
                rust: &Self::RustType,
            ) -> alloy_sol_types::abi::token::WordToken {
                let mut out = alloy_sol_types::private::Vec::new();
                <Self as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    rust,
                    &mut out,
                );
                alloy_sol_types::abi::token::WordToken(
                    alloy_sol_types::private::keccak256(out),
                )
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**```solidity
struct G2Point { uint256[2] X; uint256[2] Y; }
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct G2Point {
        #[allow(missing_docs)]
        pub X: [alloy::sol_types::private::primitives::aliases::U256; 2usize],
        #[allow(missing_docs)]
        pub Y: [alloy::sol_types::private::primitives::aliases::U256; 2usize],
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = (
            alloy::sol_types::sol_data::FixedArray<
                alloy::sol_types::sol_data::Uint<256>,
                2usize,
            >,
            alloy::sol_types::sol_data::FixedArray<
                alloy::sol_types::sol_data::Uint<256>,
                2usize,
            >,
        );
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = (
            [alloy::sol_types::private::primitives::aliases::U256; 2usize],
            [alloy::sol_types::private::primitives::aliases::U256; 2usize],
        );
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<G2Point> for UnderlyingRustTuple<'_> {
            fn from(value: G2Point) -> Self {
                (value.X, value.Y)
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for G2Point {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self { X: tuple.0, Y: tuple.1 }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolValue for G2Point {
            type SolType = Self;
        }
        #[automatically_derived]
        impl alloy_sol_types::private::SolTypeValue<Self> for G2Point {
            #[inline]
            fn stv_to_tokens(&self) -> <Self as alloy_sol_types::SolType>::Token<'_> {
                (
                    <alloy::sol_types::sol_data::FixedArray<
                        alloy::sol_types::sol_data::Uint<256>,
                        2usize,
                    > as alloy_sol_types::SolType>::tokenize(&self.X),
                    <alloy::sol_types::sol_data::FixedArray<
                        alloy::sol_types::sol_data::Uint<256>,
                        2usize,
                    > as alloy_sol_types::SolType>::tokenize(&self.Y),
                )
            }
            #[inline]
            fn stv_abi_encoded_size(&self) -> usize {
                if let Some(size) = <Self as alloy_sol_types::SolType>::ENCODED_SIZE {
                    return size;
                }
                let tuple = <UnderlyingRustTuple<
                    '_,
                > as ::core::convert::From<Self>>::from(self.clone());
                <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_encoded_size(&tuple)
            }
            #[inline]
            fn stv_eip712_data_word(&self) -> alloy_sol_types::Word {
                <Self as alloy_sol_types::SolStruct>::eip712_hash_struct(self)
            }
            #[inline]
            fn stv_abi_encode_packed_to(
                &self,
                out: &mut alloy_sol_types::private::Vec<u8>,
            ) {
                let tuple = <UnderlyingRustTuple<
                    '_,
                > as ::core::convert::From<Self>>::from(self.clone());
                <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_encode_packed_to(&tuple, out)
            }
            #[inline]
            fn stv_abi_packed_encoded_size(&self) -> usize {
                if let Some(size) = <Self as alloy_sol_types::SolType>::PACKED_ENCODED_SIZE {
                    return size;
                }
                let tuple = <UnderlyingRustTuple<
                    '_,
                > as ::core::convert::From<Self>>::from(self.clone());
                <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_packed_encoded_size(&tuple)
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolType for G2Point {
            type RustType = Self;
            type Token<'a> = <UnderlyingSolTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SOL_NAME: &'static str = <Self as alloy_sol_types::SolStruct>::NAME;
            const ENCODED_SIZE: Option<usize> = <UnderlyingSolTuple<
                '_,
            > as alloy_sol_types::SolType>::ENCODED_SIZE;
            const PACKED_ENCODED_SIZE: Option<usize> = <UnderlyingSolTuple<
                '_,
            > as alloy_sol_types::SolType>::PACKED_ENCODED_SIZE;
            #[inline]
            fn valid_token(token: &Self::Token<'_>) -> bool {
                <UnderlyingSolTuple<'_> as alloy_sol_types::SolType>::valid_token(token)
            }
            #[inline]
            fn detokenize(token: Self::Token<'_>) -> Self::RustType {
                let tuple = <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::detokenize(token);
                <Self as ::core::convert::From<UnderlyingRustTuple<'_>>>::from(tuple)
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolStruct for G2Point {
            const NAME: &'static str = "G2Point";
            #[inline]
            fn eip712_root_type() -> alloy_sol_types::private::Cow<'static, str> {
                alloy_sol_types::private::Cow::Borrowed(
                    "G2Point(uint256[2] X,uint256[2] Y)",
                )
            }
            #[inline]
            fn eip712_components() -> alloy_sol_types::private::Vec<
                alloy_sol_types::private::Cow<'static, str>,
            > {
                alloy_sol_types::private::Vec::new()
            }
            #[inline]
            fn eip712_encode_type() -> alloy_sol_types::private::Cow<'static, str> {
                <Self as alloy_sol_types::SolStruct>::eip712_root_type()
            }
            #[inline]
            fn eip712_encode_data(&self) -> alloy_sol_types::private::Vec<u8> {
                [
                    <alloy::sol_types::sol_data::FixedArray<
                        alloy::sol_types::sol_data::Uint<256>,
                        2usize,
                    > as alloy_sol_types::SolType>::eip712_data_word(&self.X)
                        .0,
                    <alloy::sol_types::sol_data::FixedArray<
                        alloy::sol_types::sol_data::Uint<256>,
                        2usize,
                    > as alloy_sol_types::SolType>::eip712_data_word(&self.Y)
                        .0,
                ]
                    .concat()
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::EventTopic for G2Point {
            #[inline]
            fn topic_preimage_length(rust: &Self::RustType) -> usize {
                0usize
                    + <alloy::sol_types::sol_data::FixedArray<
                        alloy::sol_types::sol_data::Uint<256>,
                        2usize,
                    > as alloy_sol_types::EventTopic>::topic_preimage_length(&rust.X)
                    + <alloy::sol_types::sol_data::FixedArray<
                        alloy::sol_types::sol_data::Uint<256>,
                        2usize,
                    > as alloy_sol_types::EventTopic>::topic_preimage_length(&rust.Y)
            }
            #[inline]
            fn encode_topic_preimage(
                rust: &Self::RustType,
                out: &mut alloy_sol_types::private::Vec<u8>,
            ) {
                out.reserve(
                    <Self as alloy_sol_types::EventTopic>::topic_preimage_length(rust),
                );
                <alloy::sol_types::sol_data::FixedArray<
                    alloy::sol_types::sol_data::Uint<256>,
                    2usize,
                > as alloy_sol_types::EventTopic>::encode_topic_preimage(&rust.X, out);
                <alloy::sol_types::sol_data::FixedArray<
                    alloy::sol_types::sol_data::Uint<256>,
                    2usize,
                > as alloy_sol_types::EventTopic>::encode_topic_preimage(&rust.Y, out);
            }
            #[inline]
            fn encode_topic(
                rust: &Self::RustType,
            ) -> alloy_sol_types::abi::token::WordToken {
                let mut out = alloy_sol_types::private::Vec::new();
                <Self as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    rust,
                    &mut out,
                );
                alloy_sol_types::abi::token::WordToken(
                    alloy_sol_types::private::keccak256(out),
                )
            }
        }
    };
    use alloy::contract as alloy_contract;
    /**Creates a new wrapper around an on-chain [`BN254`](self) contract instance.

See the [wrapper's documentation](`BN254Instance`) for more details.*/
    #[inline]
    pub const fn new<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    >(
        address: alloy_sol_types::private::Address,
        provider: P,
    ) -> BN254Instance<T, P, N> {
        BN254Instance::<T, P, N>::new(address, provider)
    }
    /**A [`BN254`](self) instance.

Contains type-safe methods for interacting with an on-chain instance of the
[`BN254`](self) contract located at a given `address`, using a given
provider `P`.

If the contract bytecode is available (see the [`sol!`](alloy_sol_types::sol!)
documentation on how to provide it), the `deploy` and `deploy_builder` methods can
be used to deploy a new instance of the contract.

See the [module-level documentation](self) for all the available methods.*/
    #[derive(Clone)]
    pub struct BN254Instance<T, P, N = alloy_contract::private::Ethereum> {
        address: alloy_sol_types::private::Address,
        provider: P,
        _network_transport: ::core::marker::PhantomData<(N, T)>,
    }
    #[automatically_derived]
    impl<T, P, N> ::core::fmt::Debug for BN254Instance<T, P, N> {
        #[inline]
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            f.debug_tuple("BN254Instance").field(&self.address).finish()
        }
    }
    /// Instantiation and getters/setters.
    #[automatically_derived]
    impl<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    > BN254Instance<T, P, N> {
        /**Creates a new wrapper around an on-chain [`BN254`](self) contract instance.

See the [wrapper's documentation](`BN254Instance`) for more details.*/
        #[inline]
        pub const fn new(
            address: alloy_sol_types::private::Address,
            provider: P,
        ) -> Self {
            Self {
                address,
                provider,
                _network_transport: ::core::marker::PhantomData,
            }
        }
        /// Returns a reference to the address.
        #[inline]
        pub const fn address(&self) -> &alloy_sol_types::private::Address {
            &self.address
        }
        /// Sets the address.
        #[inline]
        pub fn set_address(&mut self, address: alloy_sol_types::private::Address) {
            self.address = address;
        }
        /// Sets the address and returns `self`.
        pub fn at(mut self, address: alloy_sol_types::private::Address) -> Self {
            self.set_address(address);
            self
        }
        /// Returns a reference to the provider.
        #[inline]
        pub const fn provider(&self) -> &P {
            &self.provider
        }
    }
    impl<T, P: ::core::clone::Clone, N> BN254Instance<T, &P, N> {
        /// Clones the provider and returns a new instance with the cloned provider.
        #[inline]
        pub fn with_cloned_provider(self) -> BN254Instance<T, P, N> {
            BN254Instance {
                address: self.address,
                provider: ::core::clone::Clone::clone(&self.provider),
                _network_transport: ::core::marker::PhantomData,
            }
        }
    }
    /// Function calls.
    #[automatically_derived]
    impl<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    > BN254Instance<T, P, N> {
        /// Creates a new call builder using this contract instance's provider and address.
        ///
        /// Note that the call can be any function call, not just those defined in this
        /// contract. Prefer using the other methods for building type-safe contract calls.
        pub fn call_builder<C: alloy_sol_types::SolCall>(
            &self,
            call: &C,
        ) -> alloy_contract::SolCallBuilder<T, &P, C, N> {
            alloy_contract::SolCallBuilder::new_sol(&self.provider, &self.address, call)
        }
    }
    /// Event filters.
    #[automatically_derived]
    impl<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    > BN254Instance<T, P, N> {
        /// Creates a new event filter using this contract instance's provider and address.
        ///
        /// Note that the type can be any event, not just those defined in this contract.
        /// Prefer using the other methods for building type-safe event filters.
        pub fn event_filter<E: alloy_sol_types::SolEvent>(
            &self,
        ) -> alloy_contract::Event<T, &P, E, N> {
            alloy_contract::Event::new_sol(&self.provider, &self.address)
        }
    }
}
///Module containing a contract's types and functions.
/**

```solidity
library IBLSSignatureCheckerTypes {
    struct NonSignerStakesAndSignature { uint32[] nonSignerQuorumBitmapIndices; BN254.G1Point[] nonSignerPubkeys; BN254.G1Point[] quorumApks; BN254.G2Point apkG2; BN254.G1Point sigma; uint32[] quorumApkIndices; uint32[] totalStakeIndices; uint32[][] nonSignerStakeIndices; }
}
```*/
#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets
)]
pub mod IBLSSignatureCheckerTypes {
    use super::*;
    use alloy::sol_types as alloy_sol_types;
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**```solidity
struct NonSignerStakesAndSignature { uint32[] nonSignerQuorumBitmapIndices; BN254.G1Point[] nonSignerPubkeys; BN254.G1Point[] quorumApks; BN254.G2Point apkG2; BN254.G1Point sigma; uint32[] quorumApkIndices; uint32[] totalStakeIndices; uint32[][] nonSignerStakeIndices; }
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct NonSignerStakesAndSignature {
        #[allow(missing_docs)]
        pub nonSignerQuorumBitmapIndices: alloy::sol_types::private::Vec<u32>,
        #[allow(missing_docs)]
        pub nonSignerPubkeys: alloy::sol_types::private::Vec<
            <BN254::G1Point as alloy::sol_types::SolType>::RustType,
        >,
        #[allow(missing_docs)]
        pub quorumApks: alloy::sol_types::private::Vec<
            <BN254::G1Point as alloy::sol_types::SolType>::RustType,
        >,
        #[allow(missing_docs)]
        pub apkG2: <BN254::G2Point as alloy::sol_types::SolType>::RustType,
        #[allow(missing_docs)]
        pub sigma: <BN254::G1Point as alloy::sol_types::SolType>::RustType,
        #[allow(missing_docs)]
        pub quorumApkIndices: alloy::sol_types::private::Vec<u32>,
        #[allow(missing_docs)]
        pub totalStakeIndices: alloy::sol_types::private::Vec<u32>,
        #[allow(missing_docs)]
        pub nonSignerStakeIndices: alloy::sol_types::private::Vec<
            alloy::sol_types::private::Vec<u32>,
        >,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = (
            alloy::sol_types::sol_data::Array<alloy::sol_types::sol_data::Uint<32>>,
            alloy::sol_types::sol_data::Array<BN254::G1Point>,
            alloy::sol_types::sol_data::Array<BN254::G1Point>,
            BN254::G2Point,
            BN254::G1Point,
            alloy::sol_types::sol_data::Array<alloy::sol_types::sol_data::Uint<32>>,
            alloy::sol_types::sol_data::Array<alloy::sol_types::sol_data::Uint<32>>,
            alloy::sol_types::sol_data::Array<
                alloy::sol_types::sol_data::Array<alloy::sol_types::sol_data::Uint<32>>,
            >,
        );
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = (
            alloy::sol_types::private::Vec<u32>,
            alloy::sol_types::private::Vec<
                <BN254::G1Point as alloy::sol_types::SolType>::RustType,
            >,
            alloy::sol_types::private::Vec<
                <BN254::G1Point as alloy::sol_types::SolType>::RustType,
            >,
            <BN254::G2Point as alloy::sol_types::SolType>::RustType,
            <BN254::G1Point as alloy::sol_types::SolType>::RustType,
            alloy::sol_types::private::Vec<u32>,
            alloy::sol_types::private::Vec<u32>,
            alloy::sol_types::private::Vec<alloy::sol_types::private::Vec<u32>>,
        );
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<NonSignerStakesAndSignature>
        for UnderlyingRustTuple<'_> {
            fn from(value: NonSignerStakesAndSignature) -> Self {
                (
                    value.nonSignerQuorumBitmapIndices,
                    value.nonSignerPubkeys,
                    value.quorumApks,
                    value.apkG2,
                    value.sigma,
                    value.quorumApkIndices,
                    value.totalStakeIndices,
                    value.nonSignerStakeIndices,
                )
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>>
        for NonSignerStakesAndSignature {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {
                    nonSignerQuorumBitmapIndices: tuple.0,
                    nonSignerPubkeys: tuple.1,
                    quorumApks: tuple.2,
                    apkG2: tuple.3,
                    sigma: tuple.4,
                    quorumApkIndices: tuple.5,
                    totalStakeIndices: tuple.6,
                    nonSignerStakeIndices: tuple.7,
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolValue for NonSignerStakesAndSignature {
            type SolType = Self;
        }
        #[automatically_derived]
        impl alloy_sol_types::private::SolTypeValue<Self>
        for NonSignerStakesAndSignature {
            #[inline]
            fn stv_to_tokens(&self) -> <Self as alloy_sol_types::SolType>::Token<'_> {
                (
                    <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<32>,
                    > as alloy_sol_types::SolType>::tokenize(
                        &self.nonSignerQuorumBitmapIndices,
                    ),
                    <alloy::sol_types::sol_data::Array<
                        BN254::G1Point,
                    > as alloy_sol_types::SolType>::tokenize(&self.nonSignerPubkeys),
                    <alloy::sol_types::sol_data::Array<
                        BN254::G1Point,
                    > as alloy_sol_types::SolType>::tokenize(&self.quorumApks),
                    <BN254::G2Point as alloy_sol_types::SolType>::tokenize(&self.apkG2),
                    <BN254::G1Point as alloy_sol_types::SolType>::tokenize(&self.sigma),
                    <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<32>,
                    > as alloy_sol_types::SolType>::tokenize(&self.quorumApkIndices),
                    <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<32>,
                    > as alloy_sol_types::SolType>::tokenize(&self.totalStakeIndices),
                    <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Array<
                            alloy::sol_types::sol_data::Uint<32>,
                        >,
                    > as alloy_sol_types::SolType>::tokenize(&self.nonSignerStakeIndices),
                )
            }
            #[inline]
            fn stv_abi_encoded_size(&self) -> usize {
                if let Some(size) = <Self as alloy_sol_types::SolType>::ENCODED_SIZE {
                    return size;
                }
                let tuple = <UnderlyingRustTuple<
                    '_,
                > as ::core::convert::From<Self>>::from(self.clone());
                <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_encoded_size(&tuple)
            }
            #[inline]
            fn stv_eip712_data_word(&self) -> alloy_sol_types::Word {
                <Self as alloy_sol_types::SolStruct>::eip712_hash_struct(self)
            }
            #[inline]
            fn stv_abi_encode_packed_to(
                &self,
                out: &mut alloy_sol_types::private::Vec<u8>,
            ) {
                let tuple = <UnderlyingRustTuple<
                    '_,
                > as ::core::convert::From<Self>>::from(self.clone());
                <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_encode_packed_to(&tuple, out)
            }
            #[inline]
            fn stv_abi_packed_encoded_size(&self) -> usize {
                if let Some(size) = <Self as alloy_sol_types::SolType>::PACKED_ENCODED_SIZE {
                    return size;
                }
                let tuple = <UnderlyingRustTuple<
                    '_,
                > as ::core::convert::From<Self>>::from(self.clone());
                <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_packed_encoded_size(&tuple)
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolType for NonSignerStakesAndSignature {
            type RustType = Self;
            type Token<'a> = <UnderlyingSolTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SOL_NAME: &'static str = <Self as alloy_sol_types::SolStruct>::NAME;
            const ENCODED_SIZE: Option<usize> = <UnderlyingSolTuple<
                '_,
            > as alloy_sol_types::SolType>::ENCODED_SIZE;
            const PACKED_ENCODED_SIZE: Option<usize> = <UnderlyingSolTuple<
                '_,
            > as alloy_sol_types::SolType>::PACKED_ENCODED_SIZE;
            #[inline]
            fn valid_token(token: &Self::Token<'_>) -> bool {
                <UnderlyingSolTuple<'_> as alloy_sol_types::SolType>::valid_token(token)
            }
            #[inline]
            fn detokenize(token: Self::Token<'_>) -> Self::RustType {
                let tuple = <UnderlyingSolTuple<
                    '_,
                > as alloy_sol_types::SolType>::detokenize(token);
                <Self as ::core::convert::From<UnderlyingRustTuple<'_>>>::from(tuple)
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolStruct for NonSignerStakesAndSignature {
            const NAME: &'static str = "NonSignerStakesAndSignature";
            #[inline]
            fn eip712_root_type() -> alloy_sol_types::private::Cow<'static, str> {
                alloy_sol_types::private::Cow::Borrowed(
                    "NonSignerStakesAndSignature(uint32[] nonSignerQuorumBitmapIndices,BN254.G1Point[] nonSignerPubkeys,BN254.G1Point[] quorumApks,BN254.G2Point apkG2,BN254.G1Point sigma,uint32[] quorumApkIndices,uint32[] totalStakeIndices,uint32[][] nonSignerStakeIndices)",
                )
            }
            #[inline]
            fn eip712_components() -> alloy_sol_types::private::Vec<
                alloy_sol_types::private::Cow<'static, str>,
            > {
                let mut components = alloy_sol_types::private::Vec::with_capacity(4);
                components
                    .push(
                        <BN254::G1Point as alloy_sol_types::SolStruct>::eip712_root_type(),
                    );
                components
                    .extend(
                        <BN254::G1Point as alloy_sol_types::SolStruct>::eip712_components(),
                    );
                components
                    .push(
                        <BN254::G1Point as alloy_sol_types::SolStruct>::eip712_root_type(),
                    );
                components
                    .extend(
                        <BN254::G1Point as alloy_sol_types::SolStruct>::eip712_components(),
                    );
                components
                    .push(
                        <BN254::G2Point as alloy_sol_types::SolStruct>::eip712_root_type(),
                    );
                components
                    .extend(
                        <BN254::G2Point as alloy_sol_types::SolStruct>::eip712_components(),
                    );
                components
                    .push(
                        <BN254::G1Point as alloy_sol_types::SolStruct>::eip712_root_type(),
                    );
                components
                    .extend(
                        <BN254::G1Point as alloy_sol_types::SolStruct>::eip712_components(),
                    );
                components
            }
            #[inline]
            fn eip712_encode_data(&self) -> alloy_sol_types::private::Vec<u8> {
                [
                    <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<32>,
                    > as alloy_sol_types::SolType>::eip712_data_word(
                            &self.nonSignerQuorumBitmapIndices,
                        )
                        .0,
                    <alloy::sol_types::sol_data::Array<
                        BN254::G1Point,
                    > as alloy_sol_types::SolType>::eip712_data_word(
                            &self.nonSignerPubkeys,
                        )
                        .0,
                    <alloy::sol_types::sol_data::Array<
                        BN254::G1Point,
                    > as alloy_sol_types::SolType>::eip712_data_word(&self.quorumApks)
                        .0,
                    <BN254::G2Point as alloy_sol_types::SolType>::eip712_data_word(
                            &self.apkG2,
                        )
                        .0,
                    <BN254::G1Point as alloy_sol_types::SolType>::eip712_data_word(
                            &self.sigma,
                        )
                        .0,
                    <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<32>,
                    > as alloy_sol_types::SolType>::eip712_data_word(
                            &self.quorumApkIndices,
                        )
                        .0,
                    <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<32>,
                    > as alloy_sol_types::SolType>::eip712_data_word(
                            &self.totalStakeIndices,
                        )
                        .0,
                    <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Array<
                            alloy::sol_types::sol_data::Uint<32>,
                        >,
                    > as alloy_sol_types::SolType>::eip712_data_word(
                            &self.nonSignerStakeIndices,
                        )
                        .0,
                ]
                    .concat()
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::EventTopic for NonSignerStakesAndSignature {
            #[inline]
            fn topic_preimage_length(rust: &Self::RustType) -> usize {
                0usize
                    + <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<32>,
                    > as alloy_sol_types::EventTopic>::topic_preimage_length(
                        &rust.nonSignerQuorumBitmapIndices,
                    )
                    + <alloy::sol_types::sol_data::Array<
                        BN254::G1Point,
                    > as alloy_sol_types::EventTopic>::topic_preimage_length(
                        &rust.nonSignerPubkeys,
                    )
                    + <alloy::sol_types::sol_data::Array<
                        BN254::G1Point,
                    > as alloy_sol_types::EventTopic>::topic_preimage_length(
                        &rust.quorumApks,
                    )
                    + <BN254::G2Point as alloy_sol_types::EventTopic>::topic_preimage_length(
                        &rust.apkG2,
                    )
                    + <BN254::G1Point as alloy_sol_types::EventTopic>::topic_preimage_length(
                        &rust.sigma,
                    )
                    + <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<32>,
                    > as alloy_sol_types::EventTopic>::topic_preimage_length(
                        &rust.quorumApkIndices,
                    )
                    + <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<32>,
                    > as alloy_sol_types::EventTopic>::topic_preimage_length(
                        &rust.totalStakeIndices,
                    )
                    + <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Array<
                            alloy::sol_types::sol_data::Uint<32>,
                        >,
                    > as alloy_sol_types::EventTopic>::topic_preimage_length(
                        &rust.nonSignerStakeIndices,
                    )
            }
            #[inline]
            fn encode_topic_preimage(
                rust: &Self::RustType,
                out: &mut alloy_sol_types::private::Vec<u8>,
            ) {
                out.reserve(
                    <Self as alloy_sol_types::EventTopic>::topic_preimage_length(rust),
                );
                <alloy::sol_types::sol_data::Array<
                    alloy::sol_types::sol_data::Uint<32>,
                > as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    &rust.nonSignerQuorumBitmapIndices,
                    out,
                );
                <alloy::sol_types::sol_data::Array<
                    BN254::G1Point,
                > as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    &rust.nonSignerPubkeys,
                    out,
                );
                <alloy::sol_types::sol_data::Array<
                    BN254::G1Point,
                > as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    &rust.quorumApks,
                    out,
                );
                <BN254::G2Point as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    &rust.apkG2,
                    out,
                );
                <BN254::G1Point as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    &rust.sigma,
                    out,
                );
                <alloy::sol_types::sol_data::Array<
                    alloy::sol_types::sol_data::Uint<32>,
                > as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    &rust.quorumApkIndices,
                    out,
                );
                <alloy::sol_types::sol_data::Array<
                    alloy::sol_types::sol_data::Uint<32>,
                > as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    &rust.totalStakeIndices,
                    out,
                );
                <alloy::sol_types::sol_data::Array<
                    alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<32>,
                    >,
                > as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    &rust.nonSignerStakeIndices,
                    out,
                );
            }
            #[inline]
            fn encode_topic(
                rust: &Self::RustType,
            ) -> alloy_sol_types::abi::token::WordToken {
                let mut out = alloy_sol_types::private::Vec::new();
                <Self as alloy_sol_types::EventTopic>::encode_topic_preimage(
                    rust,
                    &mut out,
                );
                alloy_sol_types::abi::token::WordToken(
                    alloy_sol_types::private::keccak256(out),
                )
            }
        }
    };
    use alloy::contract as alloy_contract;
    /**Creates a new wrapper around an on-chain [`IBLSSignatureCheckerTypes`](self) contract instance.

See the [wrapper's documentation](`IBLSSignatureCheckerTypesInstance`) for more details.*/
    #[inline]
    pub const fn new<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    >(
        address: alloy_sol_types::private::Address,
        provider: P,
    ) -> IBLSSignatureCheckerTypesInstance<T, P, N> {
        IBLSSignatureCheckerTypesInstance::<T, P, N>::new(address, provider)
    }
    /**A [`IBLSSignatureCheckerTypes`](self) instance.

Contains type-safe methods for interacting with an on-chain instance of the
[`IBLSSignatureCheckerTypes`](self) contract located at a given `address`, using a given
provider `P`.

If the contract bytecode is available (see the [`sol!`](alloy_sol_types::sol!)
documentation on how to provide it), the `deploy` and `deploy_builder` methods can
be used to deploy a new instance of the contract.

See the [module-level documentation](self) for all the available methods.*/
    #[derive(Clone)]
    pub struct IBLSSignatureCheckerTypesInstance<
        T,
        P,
        N = alloy_contract::private::Ethereum,
    > {
        address: alloy_sol_types::private::Address,
        provider: P,
        _network_transport: ::core::marker::PhantomData<(N, T)>,
    }
    #[automatically_derived]
    impl<T, P, N> ::core::fmt::Debug for IBLSSignatureCheckerTypesInstance<T, P, N> {
        #[inline]
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            f.debug_tuple("IBLSSignatureCheckerTypesInstance")
                .field(&self.address)
                .finish()
        }
    }
    /// Instantiation and getters/setters.
    #[automatically_derived]
    impl<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    > IBLSSignatureCheckerTypesInstance<T, P, N> {
        /**Creates a new wrapper around an on-chain [`IBLSSignatureCheckerTypes`](self) contract instance.

See the [wrapper's documentation](`IBLSSignatureCheckerTypesInstance`) for more details.*/
        #[inline]
        pub const fn new(
            address: alloy_sol_types::private::Address,
            provider: P,
        ) -> Self {
            Self {
                address,
                provider,
                _network_transport: ::core::marker::PhantomData,
            }
        }
        /// Returns a reference to the address.
        #[inline]
        pub const fn address(&self) -> &alloy_sol_types::private::Address {
            &self.address
        }
        /// Sets the address.
        #[inline]
        pub fn set_address(&mut self, address: alloy_sol_types::private::Address) {
            self.address = address;
        }
        /// Sets the address and returns `self`.
        pub fn at(mut self, address: alloy_sol_types::private::Address) -> Self {
            self.set_address(address);
            self
        }
        /// Returns a reference to the provider.
        #[inline]
        pub const fn provider(&self) -> &P {
            &self.provider
        }
    }
    impl<T, P: ::core::clone::Clone, N> IBLSSignatureCheckerTypesInstance<T, &P, N> {
        /// Clones the provider and returns a new instance with the cloned provider.
        #[inline]
        pub fn with_cloned_provider(self) -> IBLSSignatureCheckerTypesInstance<T, P, N> {
            IBLSSignatureCheckerTypesInstance {
                address: self.address,
                provider: ::core::clone::Clone::clone(&self.provider),
                _network_transport: ::core::marker::PhantomData,
            }
        }
    }
    /// Function calls.
    #[automatically_derived]
    impl<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    > IBLSSignatureCheckerTypesInstance<T, P, N> {
        /// Creates a new call builder using this contract instance's provider and address.
        ///
        /// Note that the call can be any function call, not just those defined in this
        /// contract. Prefer using the other methods for building type-safe contract calls.
        pub fn call_builder<C: alloy_sol_types::SolCall>(
            &self,
            call: &C,
        ) -> alloy_contract::SolCallBuilder<T, &P, C, N> {
            alloy_contract::SolCallBuilder::new_sol(&self.provider, &self.address, call)
        }
    }
    /// Event filters.
    #[automatically_derived]
    impl<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    > IBLSSignatureCheckerTypesInstance<T, P, N> {
        /// Creates a new event filter using this contract instance's provider and address.
        ///
        /// Note that the type can be any event, not just those defined in this contract.
        /// Prefer using the other methods for building type-safe event filters.
        pub fn event_filter<E: alloy_sol_types::SolEvent>(
            &self,
        ) -> alloy_contract::Event<T, &P, E, N> {
            alloy_contract::Event::new_sol(&self.provider, &self.address)
        }
    }
}
/**

Generated by the following Solidity interface...
```solidity
library BN254 {
    struct G1Point {
        uint256 X;
        uint256 Y;
    }
    struct G2Point {
        uint256[2] X;
        uint256[2] Y;
    }
}

library IBLSSignatureCheckerTypes {
    struct NonSignerStakesAndSignature {
        uint32[] nonSignerQuorumBitmapIndices;
        BN254.G1Point[] nonSignerPubkeys;
        BN254.G1Point[] quorumApks;
        BN254.G2Point apkG2;
        BN254.G1Point sigma;
        uint32[] quorumApkIndices;
        uint32[] totalStakeIndices;
        uint32[][] nonSignerStakeIndices;
    }
}

interface ArraySummation {
    error FutureBlockNumber();
    error InsufficientQuorumThreshold();
    error InvalidArguments();
    error InvalidConfiguration();
    error InvalidOperation();
    error InvalidSignature();
    error InvalidStorageUpdates();
    error InvalidTransitionIndex();
    error RevertingContext(uint256 index, address target, bytes revertData, bytes callargs);
    error StaleBlockNumber();

    event ArrayInitialized(uint256 size);
    event SumCalculated(uint256 newSum, uint256 timestamp);

    constructor(address _avsAddress, address _blsSigChecker, uint256 _arraySize, uint256 _maxValue, uint256 _seed);

    function BLOCK_STALE_MEASURE() external view returns (uint32);
    function QUORUM_THRESHOLD() external view returns (uint8);
    function THRESHOLD_DENOMINATOR() external view returns (uint8);
    function arraySize() external view returns (uint256);
    function avsAddress() external view returns (address);
    function blsSignatureChecker() external view returns (address);
    function currentSum() external view returns (uint256);
    function getArrayElement(uint256 index) external view returns (uint256);
    function getArrayLength() external view returns (uint256);
    function getFullArray() external view returns (uint256[] memory);
    function maxValue() external view returns (uint256);
    function namespace() external view returns (bytes memory);
    function resetArray(uint256 _seed) external;
    function setArrayElement(uint256 index, uint256 newValue) external;
    function stateTransitionCount() external view returns (uint256 count);
    function sum(uint256[] memory indexes) external;
    function values(uint256) external view returns (uint256);
    function verifyAndUpdate(bytes32 msgHash, bytes memory quorumNumbers, uint32 referenceBlockNumber, bytes memory storageUpdates, uint256 transitionIndex, address targetAddr, bytes4 targetFunction, IBLSSignatureCheckerTypes.NonSignerStakesAndSignature memory nonSignerStakesAndSignature) external;
}
```

...which was generated by the following JSON ABI:
```json
[
  {
    "type": "constructor",
    "inputs": [
      {
        "name": "_avsAddress",
        "type": "address",
        "internalType": "address"
      },
      {
        "name": "_blsSigChecker",
        "type": "address",
        "internalType": "address"
      },
      {
        "name": "_arraySize",
        "type": "uint256",
        "internalType": "uint256"
      },
      {
        "name": "_maxValue",
        "type": "uint256",
        "internalType": "uint256"
      },
      {
        "name": "_seed",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "stateMutability": "nonpayable"
  },
  {
    "type": "function",
    "name": "BLOCK_STALE_MEASURE",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "uint32",
        "internalType": "uint32"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "QUORUM_THRESHOLD",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "uint8",
        "internalType": "uint8"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "THRESHOLD_DENOMINATOR",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "uint8",
        "internalType": "uint8"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "arraySize",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "avsAddress",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "address",
        "internalType": "address"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "blsSignatureChecker",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "address",
        "internalType": "contract BLSSignatureChecker"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "currentSum",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "getArrayElement",
    "inputs": [
      {
        "name": "index",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "outputs": [
      {
        "name": "",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "getArrayLength",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "getFullArray",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "uint256[]",
        "internalType": "uint256[]"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "maxValue",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "namespace",
    "inputs": [],
    "outputs": [
      {
        "name": "",
        "type": "bytes",
        "internalType": "bytes"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "resetArray",
    "inputs": [
      {
        "name": "_seed",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "outputs": [],
    "stateMutability": "nonpayable"
  },
  {
    "type": "function",
    "name": "setArrayElement",
    "inputs": [
      {
        "name": "index",
        "type": "uint256",
        "internalType": "uint256"
      },
      {
        "name": "newValue",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "outputs": [],
    "stateMutability": "nonpayable"
  },
  {
    "type": "function",
    "name": "stateTransitionCount",
    "inputs": [],
    "outputs": [
      {
        "name": "count",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "sum",
    "inputs": [
      {
        "name": "indexes",
        "type": "uint256[]",
        "internalType": "uint256[]"
      }
    ],
    "outputs": [],
    "stateMutability": "nonpayable"
  },
  {
    "type": "function",
    "name": "values",
    "inputs": [
      {
        "name": "",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "outputs": [
      {
        "name": "",
        "type": "uint256",
        "internalType": "uint256"
      }
    ],
    "stateMutability": "view"
  },
  {
    "type": "function",
    "name": "verifyAndUpdate",
    "inputs": [
      {
        "name": "msgHash",
        "type": "bytes32",
        "internalType": "bytes32"
      },
      {
        "name": "quorumNumbers",
        "type": "bytes",
        "internalType": "bytes"
      },
      {
        "name": "referenceBlockNumber",
        "type": "uint32",
        "internalType": "uint32"
      },
      {
        "name": "storageUpdates",
        "type": "bytes",
        "internalType": "bytes"
      },
      {
        "name": "transitionIndex",
        "type": "uint256",
        "internalType": "uint256"
      },
      {
        "name": "targetAddr",
        "type": "address",
        "internalType": "address"
      },
      {
        "name": "targetFunction",
        "type": "bytes4",
        "internalType": "bytes4"
      },
      {
        "name": "nonSignerStakesAndSignature",
        "type": "tuple",
        "internalType": "struct IBLSSignatureCheckerTypes.NonSignerStakesAndSignature",
        "components": [
          {
            "name": "nonSignerQuorumBitmapIndices",
            "type": "uint32[]",
            "internalType": "uint32[]"
          },
          {
            "name": "nonSignerPubkeys",
            "type": "tuple[]",
            "internalType": "struct BN254.G1Point[]",
            "components": [
              {
                "name": "X",
                "type": "uint256",
                "internalType": "uint256"
              },
              {
                "name": "Y",
                "type": "uint256",
                "internalType": "uint256"
              }
            ]
          },
          {
            "name": "quorumApks",
            "type": "tuple[]",
            "internalType": "struct BN254.G1Point[]",
            "components": [
              {
                "name": "X",
                "type": "uint256",
                "internalType": "uint256"
              },
              {
                "name": "Y",
                "type": "uint256",
                "internalType": "uint256"
              }
            ]
          },
          {
            "name": "apkG2",
            "type": "tuple",
            "internalType": "struct BN254.G2Point",
            "components": [
              {
                "name": "X",
                "type": "uint256[2]",
                "internalType": "uint256[2]"
              },
              {
                "name": "Y",
                "type": "uint256[2]",
                "internalType": "uint256[2]"
              }
            ]
          },
          {
            "name": "sigma",
            "type": "tuple",
            "internalType": "struct BN254.G1Point",
            "components": [
              {
                "name": "X",
                "type": "uint256",
                "internalType": "uint256"
              },
              {
                "name": "Y",
                "type": "uint256",
                "internalType": "uint256"
              }
            ]
          },
          {
            "name": "quorumApkIndices",
            "type": "uint32[]",
            "internalType": "uint32[]"
          },
          {
            "name": "totalStakeIndices",
            "type": "uint32[]",
            "internalType": "uint32[]"
          },
          {
            "name": "nonSignerStakeIndices",
            "type": "uint32[][]",
            "internalType": "uint32[][]"
          }
        ]
      }
    ],
    "outputs": [],
    "stateMutability": "nonpayable"
  },
  {
    "type": "event",
    "name": "ArrayInitialized",
    "inputs": [
      {
        "name": "size",
        "type": "uint256",
        "indexed": false,
        "internalType": "uint256"
      }
    ],
    "anonymous": false
  },
  {
    "type": "event",
    "name": "SumCalculated",
    "inputs": [
      {
        "name": "newSum",
        "type": "uint256",
        "indexed": false,
        "internalType": "uint256"
      },
      {
        "name": "timestamp",
        "type": "uint256",
        "indexed": false,
        "internalType": "uint256"
      }
    ],
    "anonymous": false
  },
  {
    "type": "error",
    "name": "FutureBlockNumber",
    "inputs": []
  },
  {
    "type": "error",
    "name": "InsufficientQuorumThreshold",
    "inputs": []
  },
  {
    "type": "error",
    "name": "InvalidArguments",
    "inputs": []
  },
  {
    "type": "error",
    "name": "InvalidConfiguration",
    "inputs": []
  },
  {
    "type": "error",
    "name": "InvalidOperation",
    "inputs": []
  },
  {
    "type": "error",
    "name": "InvalidSignature",
    "inputs": []
  },
  {
    "type": "error",
    "name": "InvalidStorageUpdates",
    "inputs": []
  },
  {
    "type": "error",
    "name": "InvalidTransitionIndex",
    "inputs": []
  },
  {
    "type": "error",
    "name": "RevertingContext",
    "inputs": [
      {
        "name": "index",
        "type": "uint256",
        "internalType": "uint256"
      },
      {
        "name": "target",
        "type": "address",
        "internalType": "address"
      },
      {
        "name": "revertData",
        "type": "bytes",
        "internalType": "bytes"
      },
      {
        "name": "callargs",
        "type": "bytes",
        "internalType": "bytes"
      }
    ]
  },
  {
    "type": "error",
    "name": "StaleBlockNumber",
    "inputs": []
  }
]
```*/
#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets
)]
pub mod ArraySummation {
    use super::*;
    use alloy::sol_types as alloy_sol_types;
    /// The creation / init bytecode of the contract.
    ///
    /// ```text
    ///0x60e06040526042600160146101000a81548160ff021916908360ff16021790555061012c600160156101000a81548163ffffffff021916908363ffffffff16021790555034801561004e575f5ffd5b506040516132a03803806132a083398181016040528101906100709190610323565b84848073ffffffffffffffffffffffffffffffffffffffff1660808173ffffffffffffffffffffffffffffffffffffffff16815250508160015f6101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908373ffffffffffffffffffffffffffffffffffffffff16021790555060015f9054906101000a900473ffffffffffffffffffffffffffffffffffffffff166040516020016101189190610433565b6040516020818303038152906040525f9081610134919061068c565b5050505f83148061014457505f82145b1561017b576040517fc52a9bd300000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b8260a081815250508160c0818152505061019a816101a460201b60201c565b5050505050610807565b5f81036101af574290505b5f816040516020016101c1919061076a565b604051602081830303815290604052805190602001205f1c90505f5f90505b60a05181101561025457600360c0518383604051602001610202929190610783565b604051602081830303815290604052805190602001205f1c61022491906107d7565b908060018154018082558091505060019003905f5260205f20015f909190919091505580806001019150506101e0565b507fb60b9a8636a9d1f770731fdc48912bfdacb1d8e7660792c91a051bddf9d62d4d60a051604051610286919061076a565b60405180910390a15050565b5f5ffd5b5f73ffffffffffffffffffffffffffffffffffffffff82169050919050565b5f6102bf82610296565b9050919050565b6102cf816102b5565b81146102d9575f5ffd5b50565b5f815190506102ea816102c6565b92915050565b5f819050919050565b610302816102f0565b811461030c575f5ffd5b50565b5f8151905061031d816102f9565b92915050565b5f5f5f5f5f60a0868803121561033c5761033b610292565b5b5f610349888289016102dc565b955050602061035a888289016102dc565b945050604061036b8882890161030f565b935050606061037c8882890161030f565b925050608061038d8882890161030f565b9150509295509295909350565b5f8160601b9050919050565b5f6103b08261039a565b9050919050565b5f6103c1826103a6565b9050919050565b6103d96103d4826102b5565b6103b7565b82525050565b5f81905092915050565b7f6761736b696c6c657200000000000000000000000000000000000000000000005f82015250565b5f61041d6009836103df565b9150610428826103e9565b600982019050919050565b5f61043e82846103c8565b60148201915061044d82610411565b915081905092915050565b5f81519050919050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52604160045260245ffd5b7f4e487b71000000000000000000000000000000000000000000000000000000005f52602260045260245ffd5b5f60028204905060018216806104d357607f821691505b6020821081036104e6576104e561048f565b5b50919050565b5f819050815f5260205f209050919050565b5f6020601f8301049050919050565b5f82821b905092915050565b5f600883026105487fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8261050d565b610552868361050d565b95508019841693508086168417925050509392505050565b5f819050919050565b5f61058d610588610583846102f0565b61056a565b6102f0565b9050919050565b5f819050919050565b6105a683610573565b6105ba6105b282610594565b848454610519565b825550505050565b5f5f905090565b6105d16105c2565b6105dc81848461059d565b505050565b5b818110156105ff576105f45f826105c9565b6001810190506105e2565b5050565b601f82111561064457610615816104ec565b61061e846104fe565b8101602085101561062d578190505b610641610639856104fe565b8301826105e1565b50505b505050565b5f82821c905092915050565b5f6106645f1984600802610649565b1980831691505092915050565b5f61067c8383610655565b9150826002028217905092915050565b61069582610458565b67ffffffffffffffff8111156106ae576106ad610462565b5b6106b882546104bc565b6106c3828285610603565b5f60209050601f8311600181146106f4575f84156106e2578287015190505b6106ec8582610671565b865550610753565b601f198416610702866104ec565b5f5b8281101561072957848901518255600182019150602085019450602081019050610704565b868310156107465784890151610742601f891682610655565b8355505b6001600288020188555050505b505050505050565b610764816102f0565b82525050565b5f60208201905061077d5f83018461075b565b92915050565b5f6040820190506107965f83018561075b565b6107a3602083018461075b565b9392505050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601260045260245ffd5b5f6107e1826102f0565b91506107ec836102f0565b9250826107fc576107fb6107aa565b5b828206905092915050565b60805160a05160c051612a5361084d5f395f81816109590152610c7201525f8181610a5701528181610c480152610d1a01525f818161042101526107c00152612a535ff3fe608060405234801561000f575f5ffd5b5060043610610114575f3560e01c80637c015a89116100a0578063a27b23a11161006f578063a27b23a1146102bc578063da324c13146102d8578063e0f6ff43146102f6578063ef02445814610314578063f4833e201461033257610114565b80637c015a89146102465780638b97c23d146102645780638f0bb7cc1461028257806394a5c2e41461029e57610114565b8063331f2300116100e7578063331f2300146101a05780635aff4e2e146101be5780635e383d21146101da5780635e510b601461020a5780635e8b3f2d1461022857610114565b80630194db8e146101185780630849cc9914610134578063142edc7a146101525780631c178e9c14610182575b5f5ffd5b610132600480360381019061012d91906111d6565b610350565b005b61013c6103a7565b6040516101499190611239565b60405180910390f35b61016c6004803603810190610167919061127c565b6103b3565b6040516101799190611239565b60405180910390f35b61018a61041f565b6040516101979190611321565b60405180910390f35b6101a8610443565b6040516101b591906113f1565b60405180910390f35b6101d860048036038101906101d3919061127c565b610499565b005b6101f460048036038101906101ef919061127c565b6104fb565b6040516102019190611239565b60405180910390f35b61021261051b565b60405161021f919061142c565b60405180910390f35b61023061052e565b60405161023d9190611463565b60405180910390f35b61024e610544565b60405161025b91906114ec565b60405180910390f35b61026c6105cf565b6040516102799190611239565b60405180910390f35b61029c60048036038101906102979190611671565b6105d5565b005b6102a6610957565b6040516102b39190611239565b60405180910390f35b6102d660048036038101906102d1919061177e565b61097b565b005b6102e0610a30565b6040516102ed91906117cb565b60405180910390f35b6102fe610a55565b60405161030b9190611239565b60405180910390f35b61031c610a79565b604051610329919061142c565b60405180910390f35b61033a610a7e565b6040516103479190611239565b60405180910390f35b7fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf54806001017fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf55506103a38282610aa6565b5050565b5f600380549050905090565b5f60038054905082106103fb576040517f08c379a00000000000000000000000000000000000000000000000000000000081526004016103f29061183e565b60405180910390fd5b6003828154811061040f5761040e61185c565b5b905f5260205f2001549050919050565b7f000000000000000000000000000000000000000000000000000000000000000081565b6060600380548060200260200160405190810160405280929190818152602001828054801561048f57602002820191905f5260205f20905b81548152602001906001019080831161047b575b5050505050905090565b7fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf54806001017fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf555060035f6104ef919061112b565b6104f881610c0a565b50565b6003818154811061050a575f80fd5b905f5260205f20015f915090505481565b600160149054906101000a900460ff1681565b600160159054906101000a900463ffffffff1681565b5f8054610550906118b6565b80601f016020809104026020016040519081016040528092919081815260200182805461057c906118b6565b80156105c75780601f1061059e576101008083540402835291602001916105c7565b820191905f5260205f20905b8154815290600101906020018083116105aa57829003601f168201915b505050505081565b60025481565b7fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf54806001017fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf5550438763ffffffff161061065d576040517f252f8a0e00000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b4363ffffffff16600160159054906101000a900463ffffffff16886106829190611913565b63ffffffff1610156106c0576040517f305c3e9300000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b6106c8610a7e565b6001856106d5919061194a565b1461070c576040517f7376e0a200000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b5f60028585858a8a6040516020016107289594939291906119c6565b6040516020818303038152906040526040516107449190611a4c565b602060405180830381855afa15801561075f573d5f5f3e3d5ffd5b5050506040513d601f19601f820116820180604052508101906107829190611a76565b90508a81146107bd576040517f8baa579f00000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b5f7f000000000000000000000000000000000000000000000000000000000000000073ffffffffffffffffffffffffffffffffffffffff16636efb46368d8d8d8d886040518663ffffffff1660e01b815260040161081f959493929190611fcc565b5f60405180830381865afa158015610839573d5f5f3e3d5ffd5b505050506040513d5f823e3d601f19601f820116820180604052508101906108619190612225565b5090505f5f90505b8b8b905081101561093e57600160149054906101000a900460ff1660ff168260200151828151811061089e5761089d61185c565b5b60200260200101516108b0919061227f565b6bffffffffffffffffffffffff16606460ff16835f015183815181106108d9576108d861185c565b5b60200260200101516108eb919061227f565b6bffffffffffffffffffffffff161015610931576040517f6d8605db00000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b8080600101915050610869565b506109498888610d52565b505050505050505050505050565b7f000000000000000000000000000000000000000000000000000000000000000081565b7fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf54806001017fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf55506003805490508210610a0b576040517f08c379a0000000000000000000000000000000000000000000000000000000008152600401610a029061183e565b60405180910390fd5b8060038381548110610a2057610a1f61185c565b5b905f5260205f2001819055505050565b60015f9054906101000a900473ffffffffffffffffffffffffffffffffffffffff1681565b7f000000000000000000000000000000000000000000000000000000000000000081565b606481565b5f7fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf54905090565b5f5f90505f8383905003610b03575f5f90505b600380549050811015610afd5760038181548110610ada57610ad961185c565b5b905f5260205f20015482610aee919061194a565b91508080600101915050610ab9565b50610bc5565b5f5f90505b83839050811015610bc357600380549050848483818110610b2c57610b2b61185c565b5b9050602002013510610b73576040517f08c379a0000000000000000000000000000000000000000000000000000000008152600401610b6a9061183e565b60405180910390fd5b6003848483818110610b8857610b8761185c565b5b9050602002013581548110610ba057610b9f61185c565b5b905f5260205f20015482610bb4919061194a565b91508080600101915050610b08565b505b806002819055507ffd3dfbb3da06b2710848916c65866a3d0e050047402579a6e1714261137c19c68142604051610bfd9291906122bb565b60405180910390a1505050565b5f8103610c15574290505b5f81604051602001610c279190611239565b604051602081830303815290604052805190602001205f1c90505f5f90505b7f0000000000000000000000000000000000000000000000000000000000000000811015610cf65760037f00000000000000000000000000000000000000000000000000000000000000008383604051602001610ca49291906122bb565b604051602081830303815290604052805190602001205f1c610cc6919061230f565b908060018154018082558091505060019003905f5260205f20015f90919091909150558080600101915050610c46565b507fb60b9a8636a9d1f770731fdc48912bfdacb1d8e7660792c91a051bddf9d62d4d7f0000000000000000000000000000000000000000000000000000000000000000604051610d469190611239565b60405180910390a15050565b5f5f8383810190610d6391906125a2565b91509150610d718282610d77565b50505050565b8051825114610db2576040517f5f6f132c00000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b5f5f90505b8251811015611126575f838281518110610dd457610dd361185c565b5b602002602001015190505f838381518110610df257610df161185c565b5b602002602001015190505f6006811115610e0f57610e0e612618565b5b826006811115610e2257610e21612618565b5b03610e4b575f5f82806020019051810190610e3d9190612645565b915091508082555050611117565b60016006811115610e5f57610e5e612618565b5b826006811115610e7257610e71612618565b5b03610f54575f5f5f83806020019051810190610e8e9190612740565b9250925092505f5f5a90505f5f845160208601878986f1915081610f4a575f3d90505f8167ffffffffffffffff811115610ecb57610eca612023565b5b6040519080825280601f01601f191660200182016040528015610efd5781602001600182028036833780820191505090505b509050815f602083013e898782876040517f493f09c4000000000000000000000000000000000000000000000000000000008152600401610f4194939291906127ac565b60405180910390fd5b5050505050611116565b60026006811115610f6857610f67612618565b5b826006811115610f7b57610f7a612618565b5b03610fa4575f81806020019051810190610f9591906127fd565b9050805160208201a050611115565b60036006811115610fb857610fb7612618565b5b826006811115610fcb57610fca612618565b5b03610ff9575f5f82806020019051810190610fe69190612844565b9150915080825160208401a15050611114565b6004600681111561100d5761100c612618565b5b8260068111156110205761101f612618565b5b03611053575f5f5f8380602001905181019061103c919061289e565b9250925092508082845160208601a2505050611113565b6005600681111561106757611066612618565b5b82600681111561107a57611079612618565b5b036110b2575f5f5f5f84806020019051810190611097919061290a565b9350935093509350808284865160208801a350505050611112565b6006808111156110c5576110c4612618565b5b8260068111156110d8576110d7612618565b5b03611111575f5f5f5f5f858060200190518101906110f6919061298a565b9450945094509450945080828486885160208a01a450505050505b5b5b5b5b5b5b50508080600101915050610db7565b505050565b5080545f8255905f5260205f20908101906111469190611149565b50565b5b80821115611160575f815f90555060010161114a565b5090565b5f604051905090565b5f5ffd5b5f5ffd5b5f5ffd5b5f5ffd5b5f5ffd5b5f5f83601f84011261119657611195611175565b5b8235905067ffffffffffffffff8111156111b3576111b2611179565b5b6020830191508360208202830111156111cf576111ce61117d565b5b9250929050565b5f5f602083850312156111ec576111eb61116d565b5b5f83013567ffffffffffffffff81111561120957611208611171565b5b61121585828601611181565b92509250509250929050565b5f819050919050565b61123381611221565b82525050565b5f60208201905061124c5f83018461122a565b92915050565b61125b81611221565b8114611265575f5ffd5b50565b5f8135905061127681611252565b92915050565b5f602082840312156112915761129061116d565b5b5f61129e84828501611268565b91505092915050565b5f73ffffffffffffffffffffffffffffffffffffffff82169050919050565b5f819050919050565b5f6112e96112e46112df846112a7565b6112c6565b6112a7565b9050919050565b5f6112fa826112cf565b9050919050565b5f61130b826112f0565b9050919050565b61131b81611301565b82525050565b5f6020820190506113345f830184611312565b92915050565b5f81519050919050565b5f82825260208201905092915050565b5f819050602082019050919050565b61136c81611221565b82525050565b5f61137d8383611363565b60208301905092915050565b5f602082019050919050565b5f61139f8261133a565b6113a98185611344565b93506113b483611354565b805f5b838110156113e45781516113cb8882611372565b97506113d683611389565b9250506001810190506113b7565b5085935050505092915050565b5f6020820190508181035f8301526114098184611395565b905092915050565b5f60ff82169050919050565b61142681611411565b82525050565b5f60208201905061143f5f83018461141d565b92915050565b5f63ffffffff82169050919050565b61145d81611445565b82525050565b5f6020820190506114765f830184611454565b92915050565b5f81519050919050565b5f82825260208201905092915050565b8281835e5f83830152505050565b5f601f19601f8301169050919050565b5f6114be8261147c565b6114c88185611486565b93506114d8818560208601611496565b6114e1816114a4565b840191505092915050565b5f6020820190508181035f83015261150481846114b4565b905092915050565b5f819050919050565b61151e8161150c565b8114611528575f5ffd5b50565b5f8135905061153981611515565b92915050565b5f5f83601f84011261155457611553611175565b5b8235905067ffffffffffffffff81111561157157611570611179565b5b60208301915083600182028301111561158d5761158c61117d565b5b9250929050565b61159d81611445565b81146115a7575f5ffd5b50565b5f813590506115b881611594565b92915050565b5f6115c8826112a7565b9050919050565b6115d8816115be565b81146115e2575f5ffd5b50565b5f813590506115f3816115cf565b92915050565b5f7fffffffff0000000000000000000000000000000000000000000000000000000082169050919050565b61162d816115f9565b8114611637575f5ffd5b50565b5f8135905061164881611624565b92915050565b5f5ffd5b5f61018082840312156116685761166761164e565b5b81905092915050565b5f5f5f5f5f5f5f5f5f5f6101008b8d0312156116905761168f61116d565b5b5f61169d8d828e0161152b565b9a505060208b013567ffffffffffffffff8111156116be576116bd611171565b5b6116ca8d828e0161153f565b995099505060406116dd8d828e016115aa565b97505060608b013567ffffffffffffffff8111156116fe576116fd611171565b5b61170a8d828e0161153f565b9650965050608061171d8d828e01611268565b94505060a061172e8d828e016115e5565b93505060c061173f8d828e0161163a565b92505060e08b013567ffffffffffffffff8111156117605761175f611171565b5b61176c8d828e01611652565b9150509295989b9194979a5092959850565b5f5f604083850312156117945761179361116d565b5b5f6117a185828601611268565b92505060206117b285828601611268565b9150509250929050565b6117c5816115be565b82525050565b5f6020820190506117de5f8301846117bc565b92915050565b5f82825260208201905092915050565b7f496e646578206f7574206f6620626f756e6473000000000000000000000000005f82015250565b5f6118286013836117e4565b9150611833826117f4565b602082019050919050565b5f6020820190508181035f8301526118558161181c565b9050919050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52603260045260245ffd5b7f4e487b71000000000000000000000000000000000000000000000000000000005f52602260045260245ffd5b5f60028204905060018216806118cd57607f821691505b6020821081036118e0576118df611889565b5b50919050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601160045260245ffd5b5f61191d82611445565b915061192883611445565b9250828201905063ffffffff811115611944576119436118e6565b5b92915050565b5f61195482611221565b915061195f83611221565b9250828201905080821115611977576119766118e6565b5b92915050565b611986816115f9565b82525050565b828183375f83830152505050565b5f6119a58385611486565b93506119b283858461198c565b6119bb836114a4565b840190509392505050565b5f6080820190506119d95f83018861122a565b6119e660208301876117bc565b6119f3604083018661197d565b8181036060830152611a0681848661199a565b90509695505050505050565b5f81905092915050565b5f611a268261147c565b611a308185611a12565b9350611a40818560208601611496565b80840191505092915050565b5f611a578284611a1c565b915081905092915050565b5f81519050611a7081611515565b92915050565b5f60208284031215611a8b57611a8a61116d565b5b5f611a9884828501611a62565b91505092915050565b611aaa8161150c565b82525050565b5f5ffd5b5f5ffd5b5f5ffd5b5f5f83356001602003843603038112611ad857611ad7611ab8565b5b83810192508235915060208301925067ffffffffffffffff821115611b0057611aff611ab0565b5b602082023603831315611b1657611b15611ab4565b5b509250929050565b5f82825260208201905092915050565b5f819050919050565b611b4081611445565b82525050565b5f611b518383611b37565b60208301905092915050565b5f611b6b60208401846115aa565b905092915050565b5f602082019050919050565b5f611b8a8385611b1e565b9350611b9582611b2e565b805f5b85811015611bcd57611baa8284611b5d565b611bb48882611b46565b9750611bbf83611b73565b925050600181019050611b98565b5085925050509392505050565b5f5f83356001602003843603038112611bf657611bf5611ab8565b5b83810192508235915060208301925067ffffffffffffffff821115611c1e57611c1d611ab0565b5b604082023603831315611c3457611c33611ab4565b5b509250929050565b5f82825260208201905092915050565b5f819050919050565b5f611c636020840184611268565b905092915050565b60408201611c7b5f830183611c55565b611c875f850182611363565b50611c956020830183611c55565b611ca26020850182611363565b50505050565b5f611cb38383611c6b565b60408301905092915050565b5f82905092915050565b5f604082019050919050565b5f611ce08385611c3c565b9350611ceb82611c4c565b805f5b85811015611d2357611d008284611cbf565b611d0a8882611ca8565b9750611d1583611cc9565b925050600181019050611cee565b5085925050509392505050565b5f82905092915050565b5f82905092915050565b82818337505050565b611d5960408383611d44565b5050565b60808201611d6d5f830183611d3a565b611d795f850182611d4d565b50611d876040830183611d3a565b611d946040850182611d4d565b50505050565b5f5f83356001602003843603038112611db657611db5611ab8565b5b83810192508235915060208301925067ffffffffffffffff821115611dde57611ddd611ab0565b5b602082023603831315611df457611df3611ab4565b5b509250929050565b5f82825260208201905092915050565b5f819050919050565b5f611e21848484611b7f565b90509392505050565b5f602082019050919050565b5f611e418385611dfc565b935083602084028501611e5384611e0c565b805f5b87811015611e98578484038952611e6d8284611abc565b611e78868284611e15565b9550611e8384611e2a565b935060208b019a505050600181019050611e56565b50829750879450505050509392505050565b5f6101808301611ebc5f840184611abc565b8583035f870152611ece838284611b7f565b92505050611edf6020840184611bda565b8583036020870152611ef2838284611cd5565b92505050611f036040840184611bda565b8583036040870152611f16838284611cd5565b92505050611f276060840184611d30565b611f346060860182611d5d565b50611f4260e0840184611cbf565b611f4f60e0860182611c6b565b50611f5e610120840184611abc565b858303610120870152611f72838284611b7f565b92505050611f84610140840184611abc565b858303610140870152611f98838284611b7f565b92505050611faa610160840184611d9a565b858303610160870152611fbe838284611e36565b925050508091505092915050565b5f608082019050611fdf5f830188611aa1565b8181036020830152611ff281868861199a565b90506120016040830185611454565b81810360608301526120138184611eaa565b90509695505050505050565b5f5ffd5b7f4e487b71000000000000000000000000000000000000000000000000000000005f52604160045260245ffd5b612059826114a4565b810181811067ffffffffffffffff8211171561207857612077612023565b5b80604052505050565b5f61208a611164565b90506120968282612050565b919050565b5f5ffd5b5f67ffffffffffffffff8211156120b9576120b8612023565b5b602082029050602081019050919050565b5f6bffffffffffffffffffffffff82169050919050565b6120ea816120ca565b81146120f4575f5ffd5b50565b5f81519050612105816120e1565b92915050565b5f61211d6121188461209f565b612081565b905080838252602082019050602084028301858111156121405761213f61117d565b5b835b81811015612169578061215588826120f7565b845260208401935050602081019050612142565b5050509392505050565b5f82601f83011261218757612186611175565b5b815161219784826020860161210b565b91505092915050565b5f604082840312156121b5576121b461201f565b5b6121bf6040612081565b90505f82015167ffffffffffffffff8111156121de576121dd61209b565b5b6121ea84828501612173565b5f83015250602082015167ffffffffffffffff81111561220d5761220c61209b565b5b61221984828501612173565b60208301525092915050565b5f5f6040838503121561223b5761223a61116d565b5b5f83015167ffffffffffffffff81111561225857612257611171565b5b612264858286016121a0565b925050602061227585828601611a62565b9150509250929050565b5f612289826120ca565b9150612294836120ca565b92508282026122a2816120ca565b91508082146122b4576122b36118e6565b5b5092915050565b5f6040820190506122ce5f83018561122a565b6122db602083018461122a565b9392505050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601260045260245ffd5b5f61231982611221565b915061232483611221565b925082612334576123336122e2565b5b828206905092915050565b5f67ffffffffffffffff82111561235957612358612023565b5b602082029050602081019050919050565b60078110612376575f5ffd5b50565b5f813590506123878161236a565b92915050565b5f61239f61239a8461233f565b612081565b905080838252602082019050602084028301858111156123c2576123c161117d565b5b835b818110156123eb57806123d78882612379565b8452602084019350506020810190506123c4565b5050509392505050565b5f82601f83011261240957612408611175565b5b813561241984826020860161238d565b91505092915050565b5f67ffffffffffffffff82111561243c5761243b612023565b5b602082029050602081019050919050565b5f5ffd5b5f67ffffffffffffffff82111561246b5761246a612023565b5b612474826114a4565b9050602081019050919050565b5f61249361248e84612451565b612081565b9050828152602081018484840111156124af576124ae61244d565b5b6124ba84828561198c565b509392505050565b5f82601f8301126124d6576124d5611175565b5b81356124e6848260208601612481565b91505092915050565b5f6125016124fc84612422565b612081565b905080838252602082019050602084028301858111156125245761252361117d565b5b835b8181101561256b57803567ffffffffffffffff81111561254957612548611175565b5b80860161255689826124c2565b85526020850194505050602081019050612526565b5050509392505050565b5f82601f83011261258957612588611175565b5b81356125998482602086016124ef565b91505092915050565b5f5f604083850312156125b8576125b761116d565b5b5f83013567ffffffffffffffff8111156125d5576125d4611171565b5b6125e1858286016123f5565b925050602083013567ffffffffffffffff81111561260257612601611171565b5b61260e85828601612575565b9150509250929050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52602160045260245ffd5b5f5f6040838503121561265b5761265a61116d565b5b5f61266885828601611a62565b925050602061267985828601611a62565b9150509250929050565b5f61268d826112a7565b9050919050565b61269d81612683565b81146126a7575f5ffd5b50565b5f815190506126b881612694565b92915050565b5f815190506126cc81611252565b92915050565b5f6126e46126df84612451565b612081565b905082815260208101848484011115612700576126ff61244d565b5b61270b848285611496565b509392505050565b5f82601f83011261272757612726611175565b5b81516127378482602086016126d2565b91505092915050565b5f5f5f606084860312156127575761275661116d565b5b5f612764868287016126aa565b9350506020612775868287016126be565b925050604084015167ffffffffffffffff81111561279657612795611171565b5b6127a286828701612713565b9150509250925092565b5f6080820190506127bf5f83018761122a565b6127cc60208301866117bc565b81810360408301526127de81856114b4565b905081810360608301526127f281846114b4565b905095945050505050565b5f602082840312156128125761281161116d565b5b5f82015167ffffffffffffffff81111561282f5761282e611171565b5b61283b84828501612713565b91505092915050565b5f5f6040838503121561285a5761285961116d565b5b5f83015167ffffffffffffffff81111561287757612876611171565b5b61288385828601612713565b925050602061289485828601611a62565b9150509250929050565b5f5f5f606084860312156128b5576128b461116d565b5b5f84015167ffffffffffffffff8111156128d2576128d1611171565b5b6128de86828701612713565b93505060206128ef86828701611a62565b925050604061290086828701611a62565b9150509250925092565b5f5f5f5f608085870312156129225761292161116d565b5b5f85015167ffffffffffffffff81111561293f5761293e611171565b5b61294b87828801612713565b945050602061295c87828801611a62565b935050604061296d87828801611a62565b925050606061297e87828801611a62565b91505092959194509250565b5f5f5f5f5f60a086880312156129a3576129a261116d565b5b5f86015167ffffffffffffffff8111156129c0576129bf611171565b5b6129cc88828901612713565b95505060206129dd88828901611a62565b94505060406129ee88828901611a62565b93505060606129ff88828901611a62565b9250506080612a1088828901611a62565b915050929550929590935056fea264697066735822122049e9130744c49ebfe682e1e4cefc4010fe66682c2e17d1c66b5a395c44576c1d64736f6c634300081c0033
    /// ```
    #[rustfmt::skip]
    #[allow(clippy::all)]
    pub static BYTECODE: alloy_sol_types::private::Bytes = alloy_sol_types::private::Bytes::from_static(
        b"`\xE0`@R`B`\x01`\x14a\x01\0\n\x81T\x81`\xFF\x02\x19\x16\x90\x83`\xFF\x16\x02\x17\x90UPa\x01,`\x01`\x15a\x01\0\n\x81T\x81c\xFF\xFF\xFF\xFF\x02\x19\x16\x90\x83c\xFF\xFF\xFF\xFF\x16\x02\x17\x90UP4\x80\x15a\0NW__\xFD[P`@Qa2\xA08\x03\x80a2\xA0\x839\x81\x81\x01`@R\x81\x01\x90a\0p\x91\x90a\x03#V[\x84\x84\x80s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16`\x80\x81s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x81RPP\x81`\x01_a\x01\0\n\x81T\x81s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x02\x19\x16\x90\x83s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x02\x17\x90UP`\x01_\x90T\x90a\x01\0\n\x90\x04s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16`@Q` \x01a\x01\x18\x91\x90a\x043V[`@Q` \x81\x83\x03\x03\x81R\x90`@R_\x90\x81a\x014\x91\x90a\x06\x8CV[PPP_\x83\x14\x80a\x01DWP_\x82\x14[\x15a\x01{W`@Q\x7F\xC5*\x9B\xD3\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[\x82`\xA0\x81\x81RPP\x81`\xC0\x81\x81RPPa\x01\x9A\x81a\x01\xA4` \x1B` \x1CV[PPPPPa\x08\x07V[_\x81\x03a\x01\xAFWB\x90P[_\x81`@Q` \x01a\x01\xC1\x91\x90a\x07jV[`@Q` \x81\x83\x03\x03\x81R\x90`@R\x80Q\x90` \x01 _\x1C\x90P__\x90P[`\xA0Q\x81\x10\x15a\x02TW`\x03`\xC0Q\x83\x83`@Q` \x01a\x02\x02\x92\x91\x90a\x07\x83V[`@Q` \x81\x83\x03\x03\x81R\x90`@R\x80Q\x90` \x01 _\x1Ca\x02$\x91\x90a\x07\xD7V[\x90\x80`\x01\x81T\x01\x80\x82U\x80\x91PP`\x01\x90\x03\x90_R` _ \x01_\x90\x91\x90\x91\x90\x91PU\x80\x80`\x01\x01\x91PPa\x01\xE0V[P\x7F\xB6\x0B\x9A\x866\xA9\xD1\xF7ps\x1F\xDCH\x91+\xFD\xAC\xB1\xD8\xE7f\x07\x92\xC9\x1A\x05\x1B\xDD\xF9\xD6-M`\xA0Q`@Qa\x02\x86\x91\x90a\x07jV[`@Q\x80\x91\x03\x90\xA1PPV[__\xFD[_s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[_a\x02\xBF\x82a\x02\x96V[\x90P\x91\x90PV[a\x02\xCF\x81a\x02\xB5V[\x81\x14a\x02\xD9W__\xFD[PV[_\x81Q\x90Pa\x02\xEA\x81a\x02\xC6V[\x92\x91PPV[_\x81\x90P\x91\x90PV[a\x03\x02\x81a\x02\xF0V[\x81\x14a\x03\x0CW__\xFD[PV[_\x81Q\x90Pa\x03\x1D\x81a\x02\xF9V[\x92\x91PPV[_____`\xA0\x86\x88\x03\x12\x15a\x03<Wa\x03;a\x02\x92V[[_a\x03I\x88\x82\x89\x01a\x02\xDCV[\x95PP` a\x03Z\x88\x82\x89\x01a\x02\xDCV[\x94PP`@a\x03k\x88\x82\x89\x01a\x03\x0FV[\x93PP``a\x03|\x88\x82\x89\x01a\x03\x0FV[\x92PP`\x80a\x03\x8D\x88\x82\x89\x01a\x03\x0FV[\x91PP\x92\x95P\x92\x95\x90\x93PV[_\x81``\x1B\x90P\x91\x90PV[_a\x03\xB0\x82a\x03\x9AV[\x90P\x91\x90PV[_a\x03\xC1\x82a\x03\xA6V[\x90P\x91\x90PV[a\x03\xD9a\x03\xD4\x82a\x02\xB5V[a\x03\xB7V[\x82RPPV[_\x81\x90P\x92\x91PPV[\x7Fgaskiller\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_\x82\x01RPV[_a\x04\x1D`\t\x83a\x03\xDFV[\x91Pa\x04(\x82a\x03\xE9V[`\t\x82\x01\x90P\x91\x90PV[_a\x04>\x82\x84a\x03\xC8V[`\x14\x82\x01\x91Pa\x04M\x82a\x04\x11V[\x91P\x81\x90P\x92\x91PPV[_\x81Q\x90P\x91\x90PV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`A`\x04R`$_\xFD[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`\"`\x04R`$_\xFD[_`\x02\x82\x04\x90P`\x01\x82\x16\x80a\x04\xD3W`\x7F\x82\x16\x91P[` \x82\x10\x81\x03a\x04\xE6Wa\x04\xE5a\x04\x8FV[[P\x91\x90PV[_\x81\x90P\x81_R` _ \x90P\x91\x90PV[_` `\x1F\x83\x01\x04\x90P\x91\x90PV[_\x82\x82\x1B\x90P\x92\x91PPV[_`\x08\x83\x02a\x05H\x7F\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82a\x05\rV[a\x05R\x86\x83a\x05\rV[\x95P\x80\x19\x84\x16\x93P\x80\x86\x16\x84\x17\x92PPP\x93\x92PPPV[_\x81\x90P\x91\x90PV[_a\x05\x8Da\x05\x88a\x05\x83\x84a\x02\xF0V[a\x05jV[a\x02\xF0V[\x90P\x91\x90PV[_\x81\x90P\x91\x90PV[a\x05\xA6\x83a\x05sV[a\x05\xBAa\x05\xB2\x82a\x05\x94V[\x84\x84Ta\x05\x19V[\x82UPPPPV[__\x90P\x90V[a\x05\xD1a\x05\xC2V[a\x05\xDC\x81\x84\x84a\x05\x9DV[PPPV[[\x81\x81\x10\x15a\x05\xFFWa\x05\xF4_\x82a\x05\xC9V[`\x01\x81\x01\x90Pa\x05\xE2V[PPV[`\x1F\x82\x11\x15a\x06DWa\x06\x15\x81a\x04\xECV[a\x06\x1E\x84a\x04\xFEV[\x81\x01` \x85\x10\x15a\x06-W\x81\x90P[a\x06Aa\x069\x85a\x04\xFEV[\x83\x01\x82a\x05\xE1V[PP[PPPV[_\x82\x82\x1C\x90P\x92\x91PPV[_a\x06d_\x19\x84`\x08\x02a\x06IV[\x19\x80\x83\x16\x91PP\x92\x91PPV[_a\x06|\x83\x83a\x06UV[\x91P\x82`\x02\x02\x82\x17\x90P\x92\x91PPV[a\x06\x95\x82a\x04XV[g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x06\xAEWa\x06\xADa\x04bV[[a\x06\xB8\x82Ta\x04\xBCV[a\x06\xC3\x82\x82\x85a\x06\x03V[_` \x90P`\x1F\x83\x11`\x01\x81\x14a\x06\xF4W_\x84\x15a\x06\xE2W\x82\x87\x01Q\x90P[a\x06\xEC\x85\x82a\x06qV[\x86UPa\x07SV[`\x1F\x19\x84\x16a\x07\x02\x86a\x04\xECV[_[\x82\x81\x10\x15a\x07)W\x84\x89\x01Q\x82U`\x01\x82\x01\x91P` \x85\x01\x94P` \x81\x01\x90Pa\x07\x04V[\x86\x83\x10\x15a\x07FW\x84\x89\x01Qa\x07B`\x1F\x89\x16\x82a\x06UV[\x83UP[`\x01`\x02\x88\x02\x01\x88UPPP[PPPPPPV[a\x07d\x81a\x02\xF0V[\x82RPPV[_` \x82\x01\x90Pa\x07}_\x83\x01\x84a\x07[V[\x92\x91PPV[_`@\x82\x01\x90Pa\x07\x96_\x83\x01\x85a\x07[V[a\x07\xA3` \x83\x01\x84a\x07[V[\x93\x92PPPV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`\x12`\x04R`$_\xFD[_a\x07\xE1\x82a\x02\xF0V[\x91Pa\x07\xEC\x83a\x02\xF0V[\x92P\x82a\x07\xFCWa\x07\xFBa\x07\xAAV[[\x82\x82\x06\x90P\x92\x91PPV[`\x80Q`\xA0Q`\xC0Qa*Sa\x08M_9_\x81\x81a\tY\x01Ra\x0Cr\x01R_\x81\x81a\nW\x01R\x81\x81a\x0CH\x01Ra\r\x1A\x01R_\x81\x81a\x04!\x01Ra\x07\xC0\x01Ra*S_\xF3\xFE`\x80`@R4\x80\x15a\0\x0FW__\xFD[P`\x046\x10a\x01\x14W_5`\xE0\x1C\x80c|\x01Z\x89\x11a\0\xA0W\x80c\xA2{#\xA1\x11a\0oW\x80c\xA2{#\xA1\x14a\x02\xBCW\x80c\xDA2L\x13\x14a\x02\xD8W\x80c\xE0\xF6\xFFC\x14a\x02\xF6W\x80c\xEF\x02DX\x14a\x03\x14W\x80c\xF4\x83> \x14a\x032Wa\x01\x14V[\x80c|\x01Z\x89\x14a\x02FW\x80c\x8B\x97\xC2=\x14a\x02dW\x80c\x8F\x0B\xB7\xCC\x14a\x02\x82W\x80c\x94\xA5\xC2\xE4\x14a\x02\x9EWa\x01\x14V[\x80c3\x1F#\0\x11a\0\xE7W\x80c3\x1F#\0\x14a\x01\xA0W\x80cZ\xFFN.\x14a\x01\xBEW\x80c^8=!\x14a\x01\xDAW\x80c^Q\x0B`\x14a\x02\nW\x80c^\x8B?-\x14a\x02(Wa\x01\x14V[\x80c\x01\x94\xDB\x8E\x14a\x01\x18W\x80c\x08I\xCC\x99\x14a\x014W\x80c\x14.\xDCz\x14a\x01RW\x80c\x1C\x17\x8E\x9C\x14a\x01\x82W[__\xFD[a\x012`\x04\x806\x03\x81\x01\x90a\x01-\x91\x90a\x11\xD6V[a\x03PV[\0[a\x01<a\x03\xA7V[`@Qa\x01I\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x01l`\x04\x806\x03\x81\x01\x90a\x01g\x91\x90a\x12|V[a\x03\xB3V[`@Qa\x01y\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x01\x8Aa\x04\x1FV[`@Qa\x01\x97\x91\x90a\x13!V[`@Q\x80\x91\x03\x90\xF3[a\x01\xA8a\x04CV[`@Qa\x01\xB5\x91\x90a\x13\xF1V[`@Q\x80\x91\x03\x90\xF3[a\x01\xD8`\x04\x806\x03\x81\x01\x90a\x01\xD3\x91\x90a\x12|V[a\x04\x99V[\0[a\x01\xF4`\x04\x806\x03\x81\x01\x90a\x01\xEF\x91\x90a\x12|V[a\x04\xFBV[`@Qa\x02\x01\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x02\x12a\x05\x1BV[`@Qa\x02\x1F\x91\x90a\x14,V[`@Q\x80\x91\x03\x90\xF3[a\x020a\x05.V[`@Qa\x02=\x91\x90a\x14cV[`@Q\x80\x91\x03\x90\xF3[a\x02Na\x05DV[`@Qa\x02[\x91\x90a\x14\xECV[`@Q\x80\x91\x03\x90\xF3[a\x02la\x05\xCFV[`@Qa\x02y\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x02\x9C`\x04\x806\x03\x81\x01\x90a\x02\x97\x91\x90a\x16qV[a\x05\xD5V[\0[a\x02\xA6a\tWV[`@Qa\x02\xB3\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x02\xD6`\x04\x806\x03\x81\x01\x90a\x02\xD1\x91\x90a\x17~V[a\t{V[\0[a\x02\xE0a\n0V[`@Qa\x02\xED\x91\x90a\x17\xCBV[`@Q\x80\x91\x03\x90\xF3[a\x02\xFEa\nUV[`@Qa\x03\x0B\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x03\x1Ca\nyV[`@Qa\x03)\x91\x90a\x14,V[`@Q\x80\x91\x03\x90\xF3[a\x03:a\n~V[`@Qa\x03G\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFT\x80`\x01\x01\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFUPa\x03\xA3\x82\x82a\n\xA6V[PPV[_`\x03\x80T\x90P\x90P\x90V[_`\x03\x80T\x90P\x82\x10a\x03\xFBW`@Q\x7F\x08\xC3y\xA0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\x03\xF2\x90a\x18>V[`@Q\x80\x91\x03\x90\xFD[`\x03\x82\x81T\x81\x10a\x04\x0FWa\x04\x0Ea\x18\\V[[\x90_R` _ \x01T\x90P\x91\x90PV[\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81V[```\x03\x80T\x80` \x02` \x01`@Q\x90\x81\x01`@R\x80\x92\x91\x90\x81\x81R` \x01\x82\x80T\x80\x15a\x04\x8FW` \x02\x82\x01\x91\x90_R` _ \x90[\x81T\x81R` \x01\x90`\x01\x01\x90\x80\x83\x11a\x04{W[PPPPP\x90P\x90V[\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFT\x80`\x01\x01\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFUP`\x03_a\x04\xEF\x91\x90a\x11+V[a\x04\xF8\x81a\x0C\nV[PV[`\x03\x81\x81T\x81\x10a\x05\nW_\x80\xFD[\x90_R` _ \x01_\x91P\x90PT\x81V[`\x01`\x14\x90T\x90a\x01\0\n\x90\x04`\xFF\x16\x81V[`\x01`\x15\x90T\x90a\x01\0\n\x90\x04c\xFF\xFF\xFF\xFF\x16\x81V[_\x80Ta\x05P\x90a\x18\xB6V[\x80`\x1F\x01` \x80\x91\x04\x02` \x01`@Q\x90\x81\x01`@R\x80\x92\x91\x90\x81\x81R` \x01\x82\x80Ta\x05|\x90a\x18\xB6V[\x80\x15a\x05\xC7W\x80`\x1F\x10a\x05\x9EWa\x01\0\x80\x83T\x04\x02\x83R\x91` \x01\x91a\x05\xC7V[\x82\x01\x91\x90_R` _ \x90[\x81T\x81R\x90`\x01\x01\x90` \x01\x80\x83\x11a\x05\xAAW\x82\x90\x03`\x1F\x16\x82\x01\x91[PPPPP\x81V[`\x02T\x81V[\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFT\x80`\x01\x01\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFUPC\x87c\xFF\xFF\xFF\xFF\x16\x10a\x06]W`@Q\x7F%/\x8A\x0E\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[Cc\xFF\xFF\xFF\xFF\x16`\x01`\x15\x90T\x90a\x01\0\n\x90\x04c\xFF\xFF\xFF\xFF\x16\x88a\x06\x82\x91\x90a\x19\x13V[c\xFF\xFF\xFF\xFF\x16\x10\x15a\x06\xC0W`@Q\x7F0\\>\x93\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[a\x06\xC8a\n~V[`\x01\x85a\x06\xD5\x91\x90a\x19JV[\x14a\x07\x0CW`@Q\x7Fsv\xE0\xA2\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[_`\x02\x85\x85\x85\x8A\x8A`@Q` \x01a\x07(\x95\x94\x93\x92\x91\x90a\x19\xC6V[`@Q` \x81\x83\x03\x03\x81R\x90`@R`@Qa\x07D\x91\x90a\x1ALV[` `@Q\x80\x83\x03\x81\x85Z\xFA\x15\x80\x15a\x07_W=__>=_\xFD[PPP`@Q=`\x1F\x19`\x1F\x82\x01\x16\x82\x01\x80`@RP\x81\x01\x90a\x07\x82\x91\x90a\x1AvV[\x90P\x8A\x81\x14a\x07\xBDW`@Q\x7F\x8B\xAAW\x9F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[_\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16cn\xFBF6\x8D\x8D\x8D\x8D\x88`@Q\x86c\xFF\xFF\xFF\xFF\x16`\xE0\x1B\x81R`\x04\x01a\x08\x1F\x95\x94\x93\x92\x91\x90a\x1F\xCCV[_`@Q\x80\x83\x03\x81\x86Z\xFA\x15\x80\x15a\x089W=__>=_\xFD[PPPP`@Q=_\x82>=`\x1F\x19`\x1F\x82\x01\x16\x82\x01\x80`@RP\x81\x01\x90a\x08a\x91\x90a\"%V[P\x90P__\x90P[\x8B\x8B\x90P\x81\x10\x15a\t>W`\x01`\x14\x90T\x90a\x01\0\n\x90\x04`\xFF\x16`\xFF\x16\x82` \x01Q\x82\x81Q\x81\x10a\x08\x9EWa\x08\x9Da\x18\\V[[` \x02` \x01\x01Qa\x08\xB0\x91\x90a\"\x7FV[k\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16`d`\xFF\x16\x83_\x01Q\x83\x81Q\x81\x10a\x08\xD9Wa\x08\xD8a\x18\\V[[` \x02` \x01\x01Qa\x08\xEB\x91\x90a\"\x7FV[k\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x10\x15a\t1W`@Q\x7Fm\x86\x05\xDB\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[\x80\x80`\x01\x01\x91PPa\x08iV[Pa\tI\x88\x88a\rRV[PPPPPPPPPPPPV[\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81V[\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFT\x80`\x01\x01\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFUP`\x03\x80T\x90P\x82\x10a\n\x0BW`@Q\x7F\x08\xC3y\xA0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\n\x02\x90a\x18>V[`@Q\x80\x91\x03\x90\xFD[\x80`\x03\x83\x81T\x81\x10a\n Wa\n\x1Fa\x18\\V[[\x90_R` _ \x01\x81\x90UPPPV[`\x01_\x90T\x90a\x01\0\n\x90\x04s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x81V[\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81V[`d\x81V[_\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFT\x90P\x90V[__\x90P_\x83\x83\x90P\x03a\x0B\x03W__\x90P[`\x03\x80T\x90P\x81\x10\x15a\n\xFDW`\x03\x81\x81T\x81\x10a\n\xDAWa\n\xD9a\x18\\V[[\x90_R` _ \x01T\x82a\n\xEE\x91\x90a\x19JV[\x91P\x80\x80`\x01\x01\x91PPa\n\xB9V[Pa\x0B\xC5V[__\x90P[\x83\x83\x90P\x81\x10\x15a\x0B\xC3W`\x03\x80T\x90P\x84\x84\x83\x81\x81\x10a\x0B,Wa\x0B+a\x18\\V[[\x90P` \x02\x015\x10a\x0BsW`@Q\x7F\x08\xC3y\xA0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\x0Bj\x90a\x18>V[`@Q\x80\x91\x03\x90\xFD[`\x03\x84\x84\x83\x81\x81\x10a\x0B\x88Wa\x0B\x87a\x18\\V[[\x90P` \x02\x015\x81T\x81\x10a\x0B\xA0Wa\x0B\x9Fa\x18\\V[[\x90_R` _ \x01T\x82a\x0B\xB4\x91\x90a\x19JV[\x91P\x80\x80`\x01\x01\x91PPa\x0B\x08V[P[\x80`\x02\x81\x90UP\x7F\xFD=\xFB\xB3\xDA\x06\xB2q\x08H\x91le\x86j=\x0E\x05\0G@%y\xA6\xE1qBa\x13|\x19\xC6\x81B`@Qa\x0B\xFD\x92\x91\x90a\"\xBBV[`@Q\x80\x91\x03\x90\xA1PPPV[_\x81\x03a\x0C\x15WB\x90P[_\x81`@Q` \x01a\x0C'\x91\x90a\x129V[`@Q` \x81\x83\x03\x03\x81R\x90`@R\x80Q\x90` \x01 _\x1C\x90P__\x90P[\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81\x10\x15a\x0C\xF6W`\x03\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x83\x83`@Q` \x01a\x0C\xA4\x92\x91\x90a\"\xBBV[`@Q` \x81\x83\x03\x03\x81R\x90`@R\x80Q\x90` \x01 _\x1Ca\x0C\xC6\x91\x90a#\x0FV[\x90\x80`\x01\x81T\x01\x80\x82U\x80\x91PP`\x01\x90\x03\x90_R` _ \x01_\x90\x91\x90\x91\x90\x91PU\x80\x80`\x01\x01\x91PPa\x0CFV[P\x7F\xB6\x0B\x9A\x866\xA9\xD1\xF7ps\x1F\xDCH\x91+\xFD\xAC\xB1\xD8\xE7f\x07\x92\xC9\x1A\x05\x1B\xDD\xF9\xD6-M\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0`@Qa\rF\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xA1PPV[__\x83\x83\x81\x01\x90a\rc\x91\x90a%\xA2V[\x91P\x91Pa\rq\x82\x82a\rwV[PPPPV[\x80Q\x82Q\x14a\r\xB2W`@Q\x7F_o\x13,\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[__\x90P[\x82Q\x81\x10\x15a\x11&W_\x83\x82\x81Q\x81\x10a\r\xD4Wa\r\xD3a\x18\\V[[` \x02` \x01\x01Q\x90P_\x83\x83\x81Q\x81\x10a\r\xF2Wa\r\xF1a\x18\\V[[` \x02` \x01\x01Q\x90P_`\x06\x81\x11\x15a\x0E\x0FWa\x0E\x0Ea&\x18V[[\x82`\x06\x81\x11\x15a\x0E\"Wa\x0E!a&\x18V[[\x03a\x0EKW__\x82\x80` \x01\x90Q\x81\x01\x90a\x0E=\x91\x90a&EV[\x91P\x91P\x80\x82UPPa\x11\x17V[`\x01`\x06\x81\x11\x15a\x0E_Wa\x0E^a&\x18V[[\x82`\x06\x81\x11\x15a\x0ErWa\x0Eqa&\x18V[[\x03a\x0FTW___\x83\x80` \x01\x90Q\x81\x01\x90a\x0E\x8E\x91\x90a'@V[\x92P\x92P\x92P__Z\x90P__\x84Q` \x86\x01\x87\x89\x86\xF1\x91P\x81a\x0FJW_=\x90P_\x81g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x0E\xCBWa\x0E\xCAa #V[[`@Q\x90\x80\x82R\x80`\x1F\x01`\x1F\x19\x16` \x01\x82\x01`@R\x80\x15a\x0E\xFDW\x81` \x01`\x01\x82\x02\x806\x837\x80\x82\x01\x91PP\x90P[P\x90P\x81_` \x83\x01>\x89\x87\x82\x87`@Q\x7FI?\t\xC4\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\x0FA\x94\x93\x92\x91\x90a'\xACV[`@Q\x80\x91\x03\x90\xFD[PPPPPa\x11\x16V[`\x02`\x06\x81\x11\x15a\x0FhWa\x0Fga&\x18V[[\x82`\x06\x81\x11\x15a\x0F{Wa\x0Fza&\x18V[[\x03a\x0F\xA4W_\x81\x80` \x01\x90Q\x81\x01\x90a\x0F\x95\x91\x90a'\xFDV[\x90P\x80Q` \x82\x01\xA0Pa\x11\x15V[`\x03`\x06\x81\x11\x15a\x0F\xB8Wa\x0F\xB7a&\x18V[[\x82`\x06\x81\x11\x15a\x0F\xCBWa\x0F\xCAa&\x18V[[\x03a\x0F\xF9W__\x82\x80` \x01\x90Q\x81\x01\x90a\x0F\xE6\x91\x90a(DV[\x91P\x91P\x80\x82Q` \x84\x01\xA1PPa\x11\x14V[`\x04`\x06\x81\x11\x15a\x10\rWa\x10\x0Ca&\x18V[[\x82`\x06\x81\x11\x15a\x10 Wa\x10\x1Fa&\x18V[[\x03a\x10SW___\x83\x80` \x01\x90Q\x81\x01\x90a\x10<\x91\x90a(\x9EV[\x92P\x92P\x92P\x80\x82\x84Q` \x86\x01\xA2PPPa\x11\x13V[`\x05`\x06\x81\x11\x15a\x10gWa\x10fa&\x18V[[\x82`\x06\x81\x11\x15a\x10zWa\x10ya&\x18V[[\x03a\x10\xB2W____\x84\x80` \x01\x90Q\x81\x01\x90a\x10\x97\x91\x90a)\nV[\x93P\x93P\x93P\x93P\x80\x82\x84\x86Q` \x88\x01\xA3PPPPa\x11\x12V[`\x06\x80\x81\x11\x15a\x10\xC5Wa\x10\xC4a&\x18V[[\x82`\x06\x81\x11\x15a\x10\xD8Wa\x10\xD7a&\x18V[[\x03a\x11\x11W_____\x85\x80` \x01\x90Q\x81\x01\x90a\x10\xF6\x91\x90a)\x8AV[\x94P\x94P\x94P\x94P\x94P\x80\x82\x84\x86\x88Q` \x8A\x01\xA4PPPPP[[[[[[[PP\x80\x80`\x01\x01\x91PPa\r\xB7V[PPPV[P\x80T_\x82U\x90_R` _ \x90\x81\x01\x90a\x11F\x91\x90a\x11IV[PV[[\x80\x82\x11\x15a\x11`W_\x81_\x90UP`\x01\x01a\x11JV[P\x90V[_`@Q\x90P\x90V[__\xFD[__\xFD[__\xFD[__\xFD[__\xFD[__\x83`\x1F\x84\x01\x12a\x11\x96Wa\x11\x95a\x11uV[[\x825\x90Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x11\xB3Wa\x11\xB2a\x11yV[[` \x83\x01\x91P\x83` \x82\x02\x83\x01\x11\x15a\x11\xCFWa\x11\xCEa\x11}V[[\x92P\x92\x90PV[__` \x83\x85\x03\x12\x15a\x11\xECWa\x11\xEBa\x11mV[[_\x83\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x12\tWa\x12\x08a\x11qV[[a\x12\x15\x85\x82\x86\x01a\x11\x81V[\x92P\x92PP\x92P\x92\x90PV[_\x81\x90P\x91\x90PV[a\x123\x81a\x12!V[\x82RPPV[_` \x82\x01\x90Pa\x12L_\x83\x01\x84a\x12*V[\x92\x91PPV[a\x12[\x81a\x12!V[\x81\x14a\x12eW__\xFD[PV[_\x815\x90Pa\x12v\x81a\x12RV[\x92\x91PPV[_` \x82\x84\x03\x12\x15a\x12\x91Wa\x12\x90a\x11mV[[_a\x12\x9E\x84\x82\x85\x01a\x12hV[\x91PP\x92\x91PPV[_s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[_\x81\x90P\x91\x90PV[_a\x12\xE9a\x12\xE4a\x12\xDF\x84a\x12\xA7V[a\x12\xC6V[a\x12\xA7V[\x90P\x91\x90PV[_a\x12\xFA\x82a\x12\xCFV[\x90P\x91\x90PV[_a\x13\x0B\x82a\x12\xF0V[\x90P\x91\x90PV[a\x13\x1B\x81a\x13\x01V[\x82RPPV[_` \x82\x01\x90Pa\x134_\x83\x01\x84a\x13\x12V[\x92\x91PPV[_\x81Q\x90P\x91\x90PV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[_\x81\x90P` \x82\x01\x90P\x91\x90PV[a\x13l\x81a\x12!V[\x82RPPV[_a\x13}\x83\x83a\x13cV[` \x83\x01\x90P\x92\x91PPV[_` \x82\x01\x90P\x91\x90PV[_a\x13\x9F\x82a\x13:V[a\x13\xA9\x81\x85a\x13DV[\x93Pa\x13\xB4\x83a\x13TV[\x80_[\x83\x81\x10\x15a\x13\xE4W\x81Qa\x13\xCB\x88\x82a\x13rV[\x97Pa\x13\xD6\x83a\x13\x89V[\x92PP`\x01\x81\x01\x90Pa\x13\xB7V[P\x85\x93PPPP\x92\x91PPV[_` \x82\x01\x90P\x81\x81\x03_\x83\x01Ra\x14\t\x81\x84a\x13\x95V[\x90P\x92\x91PPV[_`\xFF\x82\x16\x90P\x91\x90PV[a\x14&\x81a\x14\x11V[\x82RPPV[_` \x82\x01\x90Pa\x14?_\x83\x01\x84a\x14\x1DV[\x92\x91PPV[_c\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[a\x14]\x81a\x14EV[\x82RPPV[_` \x82\x01\x90Pa\x14v_\x83\x01\x84a\x14TV[\x92\x91PPV[_\x81Q\x90P\x91\x90PV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[\x82\x81\x83^_\x83\x83\x01RPPPV[_`\x1F\x19`\x1F\x83\x01\x16\x90P\x91\x90PV[_a\x14\xBE\x82a\x14|V[a\x14\xC8\x81\x85a\x14\x86V[\x93Pa\x14\xD8\x81\x85` \x86\x01a\x14\x96V[a\x14\xE1\x81a\x14\xA4V[\x84\x01\x91PP\x92\x91PPV[_` \x82\x01\x90P\x81\x81\x03_\x83\x01Ra\x15\x04\x81\x84a\x14\xB4V[\x90P\x92\x91PPV[_\x81\x90P\x91\x90PV[a\x15\x1E\x81a\x15\x0CV[\x81\x14a\x15(W__\xFD[PV[_\x815\x90Pa\x159\x81a\x15\x15V[\x92\x91PPV[__\x83`\x1F\x84\x01\x12a\x15TWa\x15Sa\x11uV[[\x825\x90Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x15qWa\x15pa\x11yV[[` \x83\x01\x91P\x83`\x01\x82\x02\x83\x01\x11\x15a\x15\x8DWa\x15\x8Ca\x11}V[[\x92P\x92\x90PV[a\x15\x9D\x81a\x14EV[\x81\x14a\x15\xA7W__\xFD[PV[_\x815\x90Pa\x15\xB8\x81a\x15\x94V[\x92\x91PPV[_a\x15\xC8\x82a\x12\xA7V[\x90P\x91\x90PV[a\x15\xD8\x81a\x15\xBEV[\x81\x14a\x15\xE2W__\xFD[PV[_\x815\x90Pa\x15\xF3\x81a\x15\xCFV[\x92\x91PPV[_\x7F\xFF\xFF\xFF\xFF\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x82\x16\x90P\x91\x90PV[a\x16-\x81a\x15\xF9V[\x81\x14a\x167W__\xFD[PV[_\x815\x90Pa\x16H\x81a\x16$V[\x92\x91PPV[__\xFD[_a\x01\x80\x82\x84\x03\x12\x15a\x16hWa\x16ga\x16NV[[\x81\x90P\x92\x91PPV[__________a\x01\0\x8B\x8D\x03\x12\x15a\x16\x90Wa\x16\x8Fa\x11mV[[_a\x16\x9D\x8D\x82\x8E\x01a\x15+V[\x9APP` \x8B\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x16\xBEWa\x16\xBDa\x11qV[[a\x16\xCA\x8D\x82\x8E\x01a\x15?V[\x99P\x99PP`@a\x16\xDD\x8D\x82\x8E\x01a\x15\xAAV[\x97PP``\x8B\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x16\xFEWa\x16\xFDa\x11qV[[a\x17\n\x8D\x82\x8E\x01a\x15?V[\x96P\x96PP`\x80a\x17\x1D\x8D\x82\x8E\x01a\x12hV[\x94PP`\xA0a\x17.\x8D\x82\x8E\x01a\x15\xE5V[\x93PP`\xC0a\x17?\x8D\x82\x8E\x01a\x16:V[\x92PP`\xE0\x8B\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x17`Wa\x17_a\x11qV[[a\x17l\x8D\x82\x8E\x01a\x16RV[\x91PP\x92\x95\x98\x9B\x91\x94\x97\x9AP\x92\x95\x98PV[__`@\x83\x85\x03\x12\x15a\x17\x94Wa\x17\x93a\x11mV[[_a\x17\xA1\x85\x82\x86\x01a\x12hV[\x92PP` a\x17\xB2\x85\x82\x86\x01a\x12hV[\x91PP\x92P\x92\x90PV[a\x17\xC5\x81a\x15\xBEV[\x82RPPV[_` \x82\x01\x90Pa\x17\xDE_\x83\x01\x84a\x17\xBCV[\x92\x91PPV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[\x7FIndex out of bounds\0\0\0\0\0\0\0\0\0\0\0\0\0_\x82\x01RPV[_a\x18(`\x13\x83a\x17\xE4V[\x91Pa\x183\x82a\x17\xF4V[` \x82\x01\x90P\x91\x90PV[_` \x82\x01\x90P\x81\x81\x03_\x83\x01Ra\x18U\x81a\x18\x1CV[\x90P\x91\x90PV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`2`\x04R`$_\xFD[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`\"`\x04R`$_\xFD[_`\x02\x82\x04\x90P`\x01\x82\x16\x80a\x18\xCDW`\x7F\x82\x16\x91P[` \x82\x10\x81\x03a\x18\xE0Wa\x18\xDFa\x18\x89V[[P\x91\x90PV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`\x11`\x04R`$_\xFD[_a\x19\x1D\x82a\x14EV[\x91Pa\x19(\x83a\x14EV[\x92P\x82\x82\x01\x90Pc\xFF\xFF\xFF\xFF\x81\x11\x15a\x19DWa\x19Ca\x18\xE6V[[\x92\x91PPV[_a\x19T\x82a\x12!V[\x91Pa\x19_\x83a\x12!V[\x92P\x82\x82\x01\x90P\x80\x82\x11\x15a\x19wWa\x19va\x18\xE6V[[\x92\x91PPV[a\x19\x86\x81a\x15\xF9V[\x82RPPV[\x82\x81\x837_\x83\x83\x01RPPPV[_a\x19\xA5\x83\x85a\x14\x86V[\x93Pa\x19\xB2\x83\x85\x84a\x19\x8CV[a\x19\xBB\x83a\x14\xA4V[\x84\x01\x90P\x93\x92PPPV[_`\x80\x82\x01\x90Pa\x19\xD9_\x83\x01\x88a\x12*V[a\x19\xE6` \x83\x01\x87a\x17\xBCV[a\x19\xF3`@\x83\x01\x86a\x19}V[\x81\x81\x03``\x83\x01Ra\x1A\x06\x81\x84\x86a\x19\x9AV[\x90P\x96\x95PPPPPPV[_\x81\x90P\x92\x91PPV[_a\x1A&\x82a\x14|V[a\x1A0\x81\x85a\x1A\x12V[\x93Pa\x1A@\x81\x85` \x86\x01a\x14\x96V[\x80\x84\x01\x91PP\x92\x91PPV[_a\x1AW\x82\x84a\x1A\x1CV[\x91P\x81\x90P\x92\x91PPV[_\x81Q\x90Pa\x1Ap\x81a\x15\x15V[\x92\x91PPV[_` \x82\x84\x03\x12\x15a\x1A\x8BWa\x1A\x8Aa\x11mV[[_a\x1A\x98\x84\x82\x85\x01a\x1AbV[\x91PP\x92\x91PPV[a\x1A\xAA\x81a\x15\x0CV[\x82RPPV[__\xFD[__\xFD[__\xFD[__\x835`\x01` \x03\x846\x03\x03\x81\x12a\x1A\xD8Wa\x1A\xD7a\x1A\xB8V[[\x83\x81\x01\x92P\x825\x91P` \x83\x01\x92Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a\x1B\0Wa\x1A\xFFa\x1A\xB0V[[` \x82\x026\x03\x83\x13\x15a\x1B\x16Wa\x1B\x15a\x1A\xB4V[[P\x92P\x92\x90PV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[_\x81\x90P\x91\x90PV[a\x1B@\x81a\x14EV[\x82RPPV[_a\x1BQ\x83\x83a\x1B7V[` \x83\x01\x90P\x92\x91PPV[_a\x1Bk` \x84\x01\x84a\x15\xAAV[\x90P\x92\x91PPV[_` \x82\x01\x90P\x91\x90PV[_a\x1B\x8A\x83\x85a\x1B\x1EV[\x93Pa\x1B\x95\x82a\x1B.V[\x80_[\x85\x81\x10\x15a\x1B\xCDWa\x1B\xAA\x82\x84a\x1B]V[a\x1B\xB4\x88\x82a\x1BFV[\x97Pa\x1B\xBF\x83a\x1BsV[\x92PP`\x01\x81\x01\x90Pa\x1B\x98V[P\x85\x92PPP\x93\x92PPPV[__\x835`\x01` \x03\x846\x03\x03\x81\x12a\x1B\xF6Wa\x1B\xF5a\x1A\xB8V[[\x83\x81\x01\x92P\x825\x91P` \x83\x01\x92Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a\x1C\x1EWa\x1C\x1Da\x1A\xB0V[[`@\x82\x026\x03\x83\x13\x15a\x1C4Wa\x1C3a\x1A\xB4V[[P\x92P\x92\x90PV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[_\x81\x90P\x91\x90PV[_a\x1Cc` \x84\x01\x84a\x12hV[\x90P\x92\x91PPV[`@\x82\x01a\x1C{_\x83\x01\x83a\x1CUV[a\x1C\x87_\x85\x01\x82a\x13cV[Pa\x1C\x95` \x83\x01\x83a\x1CUV[a\x1C\xA2` \x85\x01\x82a\x13cV[PPPPV[_a\x1C\xB3\x83\x83a\x1CkV[`@\x83\x01\x90P\x92\x91PPV[_\x82\x90P\x92\x91PPV[_`@\x82\x01\x90P\x91\x90PV[_a\x1C\xE0\x83\x85a\x1C<V[\x93Pa\x1C\xEB\x82a\x1CLV[\x80_[\x85\x81\x10\x15a\x1D#Wa\x1D\0\x82\x84a\x1C\xBFV[a\x1D\n\x88\x82a\x1C\xA8V[\x97Pa\x1D\x15\x83a\x1C\xC9V[\x92PP`\x01\x81\x01\x90Pa\x1C\xEEV[P\x85\x92PPP\x93\x92PPPV[_\x82\x90P\x92\x91PPV[_\x82\x90P\x92\x91PPV[\x82\x81\x837PPPV[a\x1DY`@\x83\x83a\x1DDV[PPV[`\x80\x82\x01a\x1Dm_\x83\x01\x83a\x1D:V[a\x1Dy_\x85\x01\x82a\x1DMV[Pa\x1D\x87`@\x83\x01\x83a\x1D:V[a\x1D\x94`@\x85\x01\x82a\x1DMV[PPPPV[__\x835`\x01` \x03\x846\x03\x03\x81\x12a\x1D\xB6Wa\x1D\xB5a\x1A\xB8V[[\x83\x81\x01\x92P\x825\x91P` \x83\x01\x92Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a\x1D\xDEWa\x1D\xDDa\x1A\xB0V[[` \x82\x026\x03\x83\x13\x15a\x1D\xF4Wa\x1D\xF3a\x1A\xB4V[[P\x92P\x92\x90PV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[_\x81\x90P\x91\x90PV[_a\x1E!\x84\x84\x84a\x1B\x7FV[\x90P\x93\x92PPPV[_` \x82\x01\x90P\x91\x90PV[_a\x1EA\x83\x85a\x1D\xFCV[\x93P\x83` \x84\x02\x85\x01a\x1ES\x84a\x1E\x0CV[\x80_[\x87\x81\x10\x15a\x1E\x98W\x84\x84\x03\x89Ra\x1Em\x82\x84a\x1A\xBCV[a\x1Ex\x86\x82\x84a\x1E\x15V[\x95Pa\x1E\x83\x84a\x1E*V[\x93P` \x8B\x01\x9APPP`\x01\x81\x01\x90Pa\x1EVV[P\x82\x97P\x87\x94PPPPP\x93\x92PPPV[_a\x01\x80\x83\x01a\x1E\xBC_\x84\x01\x84a\x1A\xBCV[\x85\x83\x03_\x87\x01Ra\x1E\xCE\x83\x82\x84a\x1B\x7FV[\x92PPPa\x1E\xDF` \x84\x01\x84a\x1B\xDAV[\x85\x83\x03` \x87\x01Ra\x1E\xF2\x83\x82\x84a\x1C\xD5V[\x92PPPa\x1F\x03`@\x84\x01\x84a\x1B\xDAV[\x85\x83\x03`@\x87\x01Ra\x1F\x16\x83\x82\x84a\x1C\xD5V[\x92PPPa\x1F'``\x84\x01\x84a\x1D0V[a\x1F4``\x86\x01\x82a\x1D]V[Pa\x1FB`\xE0\x84\x01\x84a\x1C\xBFV[a\x1FO`\xE0\x86\x01\x82a\x1CkV[Pa\x1F^a\x01 \x84\x01\x84a\x1A\xBCV[\x85\x83\x03a\x01 \x87\x01Ra\x1Fr\x83\x82\x84a\x1B\x7FV[\x92PPPa\x1F\x84a\x01@\x84\x01\x84a\x1A\xBCV[\x85\x83\x03a\x01@\x87\x01Ra\x1F\x98\x83\x82\x84a\x1B\x7FV[\x92PPPa\x1F\xAAa\x01`\x84\x01\x84a\x1D\x9AV[\x85\x83\x03a\x01`\x87\x01Ra\x1F\xBE\x83\x82\x84a\x1E6V[\x92PPP\x80\x91PP\x92\x91PPV[_`\x80\x82\x01\x90Pa\x1F\xDF_\x83\x01\x88a\x1A\xA1V[\x81\x81\x03` \x83\x01Ra\x1F\xF2\x81\x86\x88a\x19\x9AV[\x90Pa \x01`@\x83\x01\x85a\x14TV[\x81\x81\x03``\x83\x01Ra \x13\x81\x84a\x1E\xAAV[\x90P\x96\x95PPPPPPV[__\xFD[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`A`\x04R`$_\xFD[a Y\x82a\x14\xA4V[\x81\x01\x81\x81\x10g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x17\x15a xWa wa #V[[\x80`@RPPPV[_a \x8Aa\x11dV[\x90Pa \x96\x82\x82a PV[\x91\x90PV[__\xFD[_g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a \xB9Wa \xB8a #V[[` \x82\x02\x90P` \x81\x01\x90P\x91\x90PV[_k\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[a \xEA\x81a \xCAV[\x81\x14a \xF4W__\xFD[PV[_\x81Q\x90Pa!\x05\x81a \xE1V[\x92\x91PPV[_a!\x1Da!\x18\x84a \x9FV[a \x81V[\x90P\x80\x83\x82R` \x82\x01\x90P` \x84\x02\x83\x01\x85\x81\x11\x15a!@Wa!?a\x11}V[[\x83[\x81\x81\x10\x15a!iW\x80a!U\x88\x82a \xF7V[\x84R` \x84\x01\x93PP` \x81\x01\x90Pa!BV[PPP\x93\x92PPPV[_\x82`\x1F\x83\x01\x12a!\x87Wa!\x86a\x11uV[[\x81Qa!\x97\x84\x82` \x86\x01a!\x0BV[\x91PP\x92\x91PPV[_`@\x82\x84\x03\x12\x15a!\xB5Wa!\xB4a \x1FV[[a!\xBF`@a \x81V[\x90P_\x82\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a!\xDEWa!\xDDa \x9BV[[a!\xEA\x84\x82\x85\x01a!sV[_\x83\x01RP` \x82\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\"\rWa\"\x0Ca \x9BV[[a\"\x19\x84\x82\x85\x01a!sV[` \x83\x01RP\x92\x91PPV[__`@\x83\x85\x03\x12\x15a\";Wa\":a\x11mV[[_\x83\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\"XWa\"Wa\x11qV[[a\"d\x85\x82\x86\x01a!\xA0V[\x92PP` a\"u\x85\x82\x86\x01a\x1AbV[\x91PP\x92P\x92\x90PV[_a\"\x89\x82a \xCAV[\x91Pa\"\x94\x83a \xCAV[\x92P\x82\x82\x02a\"\xA2\x81a \xCAV[\x91P\x80\x82\x14a\"\xB4Wa\"\xB3a\x18\xE6V[[P\x92\x91PPV[_`@\x82\x01\x90Pa\"\xCE_\x83\x01\x85a\x12*V[a\"\xDB` \x83\x01\x84a\x12*V[\x93\x92PPPV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`\x12`\x04R`$_\xFD[_a#\x19\x82a\x12!V[\x91Pa#$\x83a\x12!V[\x92P\x82a#4Wa#3a\"\xE2V[[\x82\x82\x06\x90P\x92\x91PPV[_g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a#YWa#Xa #V[[` \x82\x02\x90P` \x81\x01\x90P\x91\x90PV[`\x07\x81\x10a#vW__\xFD[PV[_\x815\x90Pa#\x87\x81a#jV[\x92\x91PPV[_a#\x9Fa#\x9A\x84a#?V[a \x81V[\x90P\x80\x83\x82R` \x82\x01\x90P` \x84\x02\x83\x01\x85\x81\x11\x15a#\xC2Wa#\xC1a\x11}V[[\x83[\x81\x81\x10\x15a#\xEBW\x80a#\xD7\x88\x82a#yV[\x84R` \x84\x01\x93PP` \x81\x01\x90Pa#\xC4V[PPP\x93\x92PPPV[_\x82`\x1F\x83\x01\x12a$\tWa$\x08a\x11uV[[\x815a$\x19\x84\x82` \x86\x01a#\x8DV[\x91PP\x92\x91PPV[_g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a$<Wa$;a #V[[` \x82\x02\x90P` \x81\x01\x90P\x91\x90PV[__\xFD[_g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a$kWa$ja #V[[a$t\x82a\x14\xA4V[\x90P` \x81\x01\x90P\x91\x90PV[_a$\x93a$\x8E\x84a$QV[a \x81V[\x90P\x82\x81R` \x81\x01\x84\x84\x84\x01\x11\x15a$\xAFWa$\xAEa$MV[[a$\xBA\x84\x82\x85a\x19\x8CV[P\x93\x92PPPV[_\x82`\x1F\x83\x01\x12a$\xD6Wa$\xD5a\x11uV[[\x815a$\xE6\x84\x82` \x86\x01a$\x81V[\x91PP\x92\x91PPV[_a%\x01a$\xFC\x84a$\"V[a \x81V[\x90P\x80\x83\x82R` \x82\x01\x90P` \x84\x02\x83\x01\x85\x81\x11\x15a%$Wa%#a\x11}V[[\x83[\x81\x81\x10\x15a%kW\x805g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a%IWa%Ha\x11uV[[\x80\x86\x01a%V\x89\x82a$\xC2V[\x85R` \x85\x01\x94PPP` \x81\x01\x90Pa%&V[PPP\x93\x92PPPV[_\x82`\x1F\x83\x01\x12a%\x89Wa%\x88a\x11uV[[\x815a%\x99\x84\x82` \x86\x01a$\xEFV[\x91PP\x92\x91PPV[__`@\x83\x85\x03\x12\x15a%\xB8Wa%\xB7a\x11mV[[_\x83\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a%\xD5Wa%\xD4a\x11qV[[a%\xE1\x85\x82\x86\x01a#\xF5V[\x92PP` \x83\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a&\x02Wa&\x01a\x11qV[[a&\x0E\x85\x82\x86\x01a%uV[\x91PP\x92P\x92\x90PV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`!`\x04R`$_\xFD[__`@\x83\x85\x03\x12\x15a&[Wa&Za\x11mV[[_a&h\x85\x82\x86\x01a\x1AbV[\x92PP` a&y\x85\x82\x86\x01a\x1AbV[\x91PP\x92P\x92\x90PV[_a&\x8D\x82a\x12\xA7V[\x90P\x91\x90PV[a&\x9D\x81a&\x83V[\x81\x14a&\xA7W__\xFD[PV[_\x81Q\x90Pa&\xB8\x81a&\x94V[\x92\x91PPV[_\x81Q\x90Pa&\xCC\x81a\x12RV[\x92\x91PPV[_a&\xE4a&\xDF\x84a$QV[a \x81V[\x90P\x82\x81R` \x81\x01\x84\x84\x84\x01\x11\x15a'\0Wa&\xFFa$MV[[a'\x0B\x84\x82\x85a\x14\x96V[P\x93\x92PPPV[_\x82`\x1F\x83\x01\x12a''Wa'&a\x11uV[[\x81Qa'7\x84\x82` \x86\x01a&\xD2V[\x91PP\x92\x91PPV[___``\x84\x86\x03\x12\x15a'WWa'Va\x11mV[[_a'd\x86\x82\x87\x01a&\xAAV[\x93PP` a'u\x86\x82\x87\x01a&\xBEV[\x92PP`@\x84\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a'\x96Wa'\x95a\x11qV[[a'\xA2\x86\x82\x87\x01a'\x13V[\x91PP\x92P\x92P\x92V[_`\x80\x82\x01\x90Pa'\xBF_\x83\x01\x87a\x12*V[a'\xCC` \x83\x01\x86a\x17\xBCV[\x81\x81\x03`@\x83\x01Ra'\xDE\x81\x85a\x14\xB4V[\x90P\x81\x81\x03``\x83\x01Ra'\xF2\x81\x84a\x14\xB4V[\x90P\x95\x94PPPPPV[_` \x82\x84\x03\x12\x15a(\x12Wa(\x11a\x11mV[[_\x82\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a(/Wa(.a\x11qV[[a(;\x84\x82\x85\x01a'\x13V[\x91PP\x92\x91PPV[__`@\x83\x85\x03\x12\x15a(ZWa(Ya\x11mV[[_\x83\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a(wWa(va\x11qV[[a(\x83\x85\x82\x86\x01a'\x13V[\x92PP` a(\x94\x85\x82\x86\x01a\x1AbV[\x91PP\x92P\x92\x90PV[___``\x84\x86\x03\x12\x15a(\xB5Wa(\xB4a\x11mV[[_\x84\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a(\xD2Wa(\xD1a\x11qV[[a(\xDE\x86\x82\x87\x01a'\x13V[\x93PP` a(\xEF\x86\x82\x87\x01a\x1AbV[\x92PP`@a)\0\x86\x82\x87\x01a\x1AbV[\x91PP\x92P\x92P\x92V[____`\x80\x85\x87\x03\x12\x15a)\"Wa)!a\x11mV[[_\x85\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a)?Wa)>a\x11qV[[a)K\x87\x82\x88\x01a'\x13V[\x94PP` a)\\\x87\x82\x88\x01a\x1AbV[\x93PP`@a)m\x87\x82\x88\x01a\x1AbV[\x92PP``a)~\x87\x82\x88\x01a\x1AbV[\x91PP\x92\x95\x91\x94P\x92PV[_____`\xA0\x86\x88\x03\x12\x15a)\xA3Wa)\xA2a\x11mV[[_\x86\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a)\xC0Wa)\xBFa\x11qV[[a)\xCC\x88\x82\x89\x01a'\x13V[\x95PP` a)\xDD\x88\x82\x89\x01a\x1AbV[\x94PP`@a)\xEE\x88\x82\x89\x01a\x1AbV[\x93PP``a)\xFF\x88\x82\x89\x01a\x1AbV[\x92PP`\x80a*\x10\x88\x82\x89\x01a\x1AbV[\x91PP\x92\x95P\x92\x95\x90\x93PV\xFE\xA2dipfsX\"\x12 I\xE9\x13\x07D\xC4\x9E\xBF\xE6\x82\xE1\xE4\xCE\xFC@\x10\xFEfh,.\x17\xD1\xC6kZ9\\DWl\x1DdsolcC\0\x08\x1C\x003",
    );
    /// The runtime bytecode of the contract, as deployed on the network.
    ///
    /// ```text
    ///0x608060405234801561000f575f5ffd5b5060043610610114575f3560e01c80637c015a89116100a0578063a27b23a11161006f578063a27b23a1146102bc578063da324c13146102d8578063e0f6ff43146102f6578063ef02445814610314578063f4833e201461033257610114565b80637c015a89146102465780638b97c23d146102645780638f0bb7cc1461028257806394a5c2e41461029e57610114565b8063331f2300116100e7578063331f2300146101a05780635aff4e2e146101be5780635e383d21146101da5780635e510b601461020a5780635e8b3f2d1461022857610114565b80630194db8e146101185780630849cc9914610134578063142edc7a146101525780631c178e9c14610182575b5f5ffd5b610132600480360381019061012d91906111d6565b610350565b005b61013c6103a7565b6040516101499190611239565b60405180910390f35b61016c6004803603810190610167919061127c565b6103b3565b6040516101799190611239565b60405180910390f35b61018a61041f565b6040516101979190611321565b60405180910390f35b6101a8610443565b6040516101b591906113f1565b60405180910390f35b6101d860048036038101906101d3919061127c565b610499565b005b6101f460048036038101906101ef919061127c565b6104fb565b6040516102019190611239565b60405180910390f35b61021261051b565b60405161021f919061142c565b60405180910390f35b61023061052e565b60405161023d9190611463565b60405180910390f35b61024e610544565b60405161025b91906114ec565b60405180910390f35b61026c6105cf565b6040516102799190611239565b60405180910390f35b61029c60048036038101906102979190611671565b6105d5565b005b6102a6610957565b6040516102b39190611239565b60405180910390f35b6102d660048036038101906102d1919061177e565b61097b565b005b6102e0610a30565b6040516102ed91906117cb565b60405180910390f35b6102fe610a55565b60405161030b9190611239565b60405180910390f35b61031c610a79565b604051610329919061142c565b60405180910390f35b61033a610a7e565b6040516103479190611239565b60405180910390f35b7fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf54806001017fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf55506103a38282610aa6565b5050565b5f600380549050905090565b5f60038054905082106103fb576040517f08c379a00000000000000000000000000000000000000000000000000000000081526004016103f29061183e565b60405180910390fd5b6003828154811061040f5761040e61185c565b5b905f5260205f2001549050919050565b7f000000000000000000000000000000000000000000000000000000000000000081565b6060600380548060200260200160405190810160405280929190818152602001828054801561048f57602002820191905f5260205f20905b81548152602001906001019080831161047b575b5050505050905090565b7fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf54806001017fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf555060035f6104ef919061112b565b6104f881610c0a565b50565b6003818154811061050a575f80fd5b905f5260205f20015f915090505481565b600160149054906101000a900460ff1681565b600160159054906101000a900463ffffffff1681565b5f8054610550906118b6565b80601f016020809104026020016040519081016040528092919081815260200182805461057c906118b6565b80156105c75780601f1061059e576101008083540402835291602001916105c7565b820191905f5260205f20905b8154815290600101906020018083116105aa57829003601f168201915b505050505081565b60025481565b7fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf54806001017fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf5550438763ffffffff161061065d576040517f252f8a0e00000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b4363ffffffff16600160159054906101000a900463ffffffff16886106829190611913565b63ffffffff1610156106c0576040517f305c3e9300000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b6106c8610a7e565b6001856106d5919061194a565b1461070c576040517f7376e0a200000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b5f60028585858a8a6040516020016107289594939291906119c6565b6040516020818303038152906040526040516107449190611a4c565b602060405180830381855afa15801561075f573d5f5f3e3d5ffd5b5050506040513d601f19601f820116820180604052508101906107829190611a76565b90508a81146107bd576040517f8baa579f00000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b5f7f000000000000000000000000000000000000000000000000000000000000000073ffffffffffffffffffffffffffffffffffffffff16636efb46368d8d8d8d886040518663ffffffff1660e01b815260040161081f959493929190611fcc565b5f60405180830381865afa158015610839573d5f5f3e3d5ffd5b505050506040513d5f823e3d601f19601f820116820180604052508101906108619190612225565b5090505f5f90505b8b8b905081101561093e57600160149054906101000a900460ff1660ff168260200151828151811061089e5761089d61185c565b5b60200260200101516108b0919061227f565b6bffffffffffffffffffffffff16606460ff16835f015183815181106108d9576108d861185c565b5b60200260200101516108eb919061227f565b6bffffffffffffffffffffffff161015610931576040517f6d8605db00000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b8080600101915050610869565b506109498888610d52565b505050505050505050505050565b7f000000000000000000000000000000000000000000000000000000000000000081565b7fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf54806001017fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf55506003805490508210610a0b576040517f08c379a0000000000000000000000000000000000000000000000000000000008152600401610a029061183e565b60405180910390fd5b8060038381548110610a2057610a1f61185c565b5b905f5260205f2001819055505050565b60015f9054906101000a900473ffffffffffffffffffffffffffffffffffffffff1681565b7f000000000000000000000000000000000000000000000000000000000000000081565b606481565b5f7fdebfdfd5a50ad117c10898d68b5ccf0893c6b40d4f443f902e2e7646601bdeaf54905090565b5f5f90505f8383905003610b03575f5f90505b600380549050811015610afd5760038181548110610ada57610ad961185c565b5b905f5260205f20015482610aee919061194a565b91508080600101915050610ab9565b50610bc5565b5f5f90505b83839050811015610bc357600380549050848483818110610b2c57610b2b61185c565b5b9050602002013510610b73576040517f08c379a0000000000000000000000000000000000000000000000000000000008152600401610b6a9061183e565b60405180910390fd5b6003848483818110610b8857610b8761185c565b5b9050602002013581548110610ba057610b9f61185c565b5b905f5260205f20015482610bb4919061194a565b91508080600101915050610b08565b505b806002819055507ffd3dfbb3da06b2710848916c65866a3d0e050047402579a6e1714261137c19c68142604051610bfd9291906122bb565b60405180910390a1505050565b5f8103610c15574290505b5f81604051602001610c279190611239565b604051602081830303815290604052805190602001205f1c90505f5f90505b7f0000000000000000000000000000000000000000000000000000000000000000811015610cf65760037f00000000000000000000000000000000000000000000000000000000000000008383604051602001610ca49291906122bb565b604051602081830303815290604052805190602001205f1c610cc6919061230f565b908060018154018082558091505060019003905f5260205f20015f90919091909150558080600101915050610c46565b507fb60b9a8636a9d1f770731fdc48912bfdacb1d8e7660792c91a051bddf9d62d4d7f0000000000000000000000000000000000000000000000000000000000000000604051610d469190611239565b60405180910390a15050565b5f5f8383810190610d6391906125a2565b91509150610d718282610d77565b50505050565b8051825114610db2576040517f5f6f132c00000000000000000000000000000000000000000000000000000000815260040160405180910390fd5b5f5f90505b8251811015611126575f838281518110610dd457610dd361185c565b5b602002602001015190505f838381518110610df257610df161185c565b5b602002602001015190505f6006811115610e0f57610e0e612618565b5b826006811115610e2257610e21612618565b5b03610e4b575f5f82806020019051810190610e3d9190612645565b915091508082555050611117565b60016006811115610e5f57610e5e612618565b5b826006811115610e7257610e71612618565b5b03610f54575f5f5f83806020019051810190610e8e9190612740565b9250925092505f5f5a90505f5f845160208601878986f1915081610f4a575f3d90505f8167ffffffffffffffff811115610ecb57610eca612023565b5b6040519080825280601f01601f191660200182016040528015610efd5781602001600182028036833780820191505090505b509050815f602083013e898782876040517f493f09c4000000000000000000000000000000000000000000000000000000008152600401610f4194939291906127ac565b60405180910390fd5b5050505050611116565b60026006811115610f6857610f67612618565b5b826006811115610f7b57610f7a612618565b5b03610fa4575f81806020019051810190610f9591906127fd565b9050805160208201a050611115565b60036006811115610fb857610fb7612618565b5b826006811115610fcb57610fca612618565b5b03610ff9575f5f82806020019051810190610fe69190612844565b9150915080825160208401a15050611114565b6004600681111561100d5761100c612618565b5b8260068111156110205761101f612618565b5b03611053575f5f5f8380602001905181019061103c919061289e565b9250925092508082845160208601a2505050611113565b6005600681111561106757611066612618565b5b82600681111561107a57611079612618565b5b036110b2575f5f5f5f84806020019051810190611097919061290a565b9350935093509350808284865160208801a350505050611112565b6006808111156110c5576110c4612618565b5b8260068111156110d8576110d7612618565b5b03611111575f5f5f5f5f858060200190518101906110f6919061298a565b9450945094509450945080828486885160208a01a450505050505b5b5b5b5b5b5b50508080600101915050610db7565b505050565b5080545f8255905f5260205f20908101906111469190611149565b50565b5b80821115611160575f815f90555060010161114a565b5090565b5f604051905090565b5f5ffd5b5f5ffd5b5f5ffd5b5f5ffd5b5f5ffd5b5f5f83601f84011261119657611195611175565b5b8235905067ffffffffffffffff8111156111b3576111b2611179565b5b6020830191508360208202830111156111cf576111ce61117d565b5b9250929050565b5f5f602083850312156111ec576111eb61116d565b5b5f83013567ffffffffffffffff81111561120957611208611171565b5b61121585828601611181565b92509250509250929050565b5f819050919050565b61123381611221565b82525050565b5f60208201905061124c5f83018461122a565b92915050565b61125b81611221565b8114611265575f5ffd5b50565b5f8135905061127681611252565b92915050565b5f602082840312156112915761129061116d565b5b5f61129e84828501611268565b91505092915050565b5f73ffffffffffffffffffffffffffffffffffffffff82169050919050565b5f819050919050565b5f6112e96112e46112df846112a7565b6112c6565b6112a7565b9050919050565b5f6112fa826112cf565b9050919050565b5f61130b826112f0565b9050919050565b61131b81611301565b82525050565b5f6020820190506113345f830184611312565b92915050565b5f81519050919050565b5f82825260208201905092915050565b5f819050602082019050919050565b61136c81611221565b82525050565b5f61137d8383611363565b60208301905092915050565b5f602082019050919050565b5f61139f8261133a565b6113a98185611344565b93506113b483611354565b805f5b838110156113e45781516113cb8882611372565b97506113d683611389565b9250506001810190506113b7565b5085935050505092915050565b5f6020820190508181035f8301526114098184611395565b905092915050565b5f60ff82169050919050565b61142681611411565b82525050565b5f60208201905061143f5f83018461141d565b92915050565b5f63ffffffff82169050919050565b61145d81611445565b82525050565b5f6020820190506114765f830184611454565b92915050565b5f81519050919050565b5f82825260208201905092915050565b8281835e5f83830152505050565b5f601f19601f8301169050919050565b5f6114be8261147c565b6114c88185611486565b93506114d8818560208601611496565b6114e1816114a4565b840191505092915050565b5f6020820190508181035f83015261150481846114b4565b905092915050565b5f819050919050565b61151e8161150c565b8114611528575f5ffd5b50565b5f8135905061153981611515565b92915050565b5f5f83601f84011261155457611553611175565b5b8235905067ffffffffffffffff81111561157157611570611179565b5b60208301915083600182028301111561158d5761158c61117d565b5b9250929050565b61159d81611445565b81146115a7575f5ffd5b50565b5f813590506115b881611594565b92915050565b5f6115c8826112a7565b9050919050565b6115d8816115be565b81146115e2575f5ffd5b50565b5f813590506115f3816115cf565b92915050565b5f7fffffffff0000000000000000000000000000000000000000000000000000000082169050919050565b61162d816115f9565b8114611637575f5ffd5b50565b5f8135905061164881611624565b92915050565b5f5ffd5b5f61018082840312156116685761166761164e565b5b81905092915050565b5f5f5f5f5f5f5f5f5f5f6101008b8d0312156116905761168f61116d565b5b5f61169d8d828e0161152b565b9a505060208b013567ffffffffffffffff8111156116be576116bd611171565b5b6116ca8d828e0161153f565b995099505060406116dd8d828e016115aa565b97505060608b013567ffffffffffffffff8111156116fe576116fd611171565b5b61170a8d828e0161153f565b9650965050608061171d8d828e01611268565b94505060a061172e8d828e016115e5565b93505060c061173f8d828e0161163a565b92505060e08b013567ffffffffffffffff8111156117605761175f611171565b5b61176c8d828e01611652565b9150509295989b9194979a5092959850565b5f5f604083850312156117945761179361116d565b5b5f6117a185828601611268565b92505060206117b285828601611268565b9150509250929050565b6117c5816115be565b82525050565b5f6020820190506117de5f8301846117bc565b92915050565b5f82825260208201905092915050565b7f496e646578206f7574206f6620626f756e6473000000000000000000000000005f82015250565b5f6118286013836117e4565b9150611833826117f4565b602082019050919050565b5f6020820190508181035f8301526118558161181c565b9050919050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52603260045260245ffd5b7f4e487b71000000000000000000000000000000000000000000000000000000005f52602260045260245ffd5b5f60028204905060018216806118cd57607f821691505b6020821081036118e0576118df611889565b5b50919050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601160045260245ffd5b5f61191d82611445565b915061192883611445565b9250828201905063ffffffff811115611944576119436118e6565b5b92915050565b5f61195482611221565b915061195f83611221565b9250828201905080821115611977576119766118e6565b5b92915050565b611986816115f9565b82525050565b828183375f83830152505050565b5f6119a58385611486565b93506119b283858461198c565b6119bb836114a4565b840190509392505050565b5f6080820190506119d95f83018861122a565b6119e660208301876117bc565b6119f3604083018661197d565b8181036060830152611a0681848661199a565b90509695505050505050565b5f81905092915050565b5f611a268261147c565b611a308185611a12565b9350611a40818560208601611496565b80840191505092915050565b5f611a578284611a1c565b915081905092915050565b5f81519050611a7081611515565b92915050565b5f60208284031215611a8b57611a8a61116d565b5b5f611a9884828501611a62565b91505092915050565b611aaa8161150c565b82525050565b5f5ffd5b5f5ffd5b5f5ffd5b5f5f83356001602003843603038112611ad857611ad7611ab8565b5b83810192508235915060208301925067ffffffffffffffff821115611b0057611aff611ab0565b5b602082023603831315611b1657611b15611ab4565b5b509250929050565b5f82825260208201905092915050565b5f819050919050565b611b4081611445565b82525050565b5f611b518383611b37565b60208301905092915050565b5f611b6b60208401846115aa565b905092915050565b5f602082019050919050565b5f611b8a8385611b1e565b9350611b9582611b2e565b805f5b85811015611bcd57611baa8284611b5d565b611bb48882611b46565b9750611bbf83611b73565b925050600181019050611b98565b5085925050509392505050565b5f5f83356001602003843603038112611bf657611bf5611ab8565b5b83810192508235915060208301925067ffffffffffffffff821115611c1e57611c1d611ab0565b5b604082023603831315611c3457611c33611ab4565b5b509250929050565b5f82825260208201905092915050565b5f819050919050565b5f611c636020840184611268565b905092915050565b60408201611c7b5f830183611c55565b611c875f850182611363565b50611c956020830183611c55565b611ca26020850182611363565b50505050565b5f611cb38383611c6b565b60408301905092915050565b5f82905092915050565b5f604082019050919050565b5f611ce08385611c3c565b9350611ceb82611c4c565b805f5b85811015611d2357611d008284611cbf565b611d0a8882611ca8565b9750611d1583611cc9565b925050600181019050611cee565b5085925050509392505050565b5f82905092915050565b5f82905092915050565b82818337505050565b611d5960408383611d44565b5050565b60808201611d6d5f830183611d3a565b611d795f850182611d4d565b50611d876040830183611d3a565b611d946040850182611d4d565b50505050565b5f5f83356001602003843603038112611db657611db5611ab8565b5b83810192508235915060208301925067ffffffffffffffff821115611dde57611ddd611ab0565b5b602082023603831315611df457611df3611ab4565b5b509250929050565b5f82825260208201905092915050565b5f819050919050565b5f611e21848484611b7f565b90509392505050565b5f602082019050919050565b5f611e418385611dfc565b935083602084028501611e5384611e0c565b805f5b87811015611e98578484038952611e6d8284611abc565b611e78868284611e15565b9550611e8384611e2a565b935060208b019a505050600181019050611e56565b50829750879450505050509392505050565b5f6101808301611ebc5f840184611abc565b8583035f870152611ece838284611b7f565b92505050611edf6020840184611bda565b8583036020870152611ef2838284611cd5565b92505050611f036040840184611bda565b8583036040870152611f16838284611cd5565b92505050611f276060840184611d30565b611f346060860182611d5d565b50611f4260e0840184611cbf565b611f4f60e0860182611c6b565b50611f5e610120840184611abc565b858303610120870152611f72838284611b7f565b92505050611f84610140840184611abc565b858303610140870152611f98838284611b7f565b92505050611faa610160840184611d9a565b858303610160870152611fbe838284611e36565b925050508091505092915050565b5f608082019050611fdf5f830188611aa1565b8181036020830152611ff281868861199a565b90506120016040830185611454565b81810360608301526120138184611eaa565b90509695505050505050565b5f5ffd5b7f4e487b71000000000000000000000000000000000000000000000000000000005f52604160045260245ffd5b612059826114a4565b810181811067ffffffffffffffff8211171561207857612077612023565b5b80604052505050565b5f61208a611164565b90506120968282612050565b919050565b5f5ffd5b5f67ffffffffffffffff8211156120b9576120b8612023565b5b602082029050602081019050919050565b5f6bffffffffffffffffffffffff82169050919050565b6120ea816120ca565b81146120f4575f5ffd5b50565b5f81519050612105816120e1565b92915050565b5f61211d6121188461209f565b612081565b905080838252602082019050602084028301858111156121405761213f61117d565b5b835b81811015612169578061215588826120f7565b845260208401935050602081019050612142565b5050509392505050565b5f82601f83011261218757612186611175565b5b815161219784826020860161210b565b91505092915050565b5f604082840312156121b5576121b461201f565b5b6121bf6040612081565b90505f82015167ffffffffffffffff8111156121de576121dd61209b565b5b6121ea84828501612173565b5f83015250602082015167ffffffffffffffff81111561220d5761220c61209b565b5b61221984828501612173565b60208301525092915050565b5f5f6040838503121561223b5761223a61116d565b5b5f83015167ffffffffffffffff81111561225857612257611171565b5b612264858286016121a0565b925050602061227585828601611a62565b9150509250929050565b5f612289826120ca565b9150612294836120ca565b92508282026122a2816120ca565b91508082146122b4576122b36118e6565b5b5092915050565b5f6040820190506122ce5f83018561122a565b6122db602083018461122a565b9392505050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601260045260245ffd5b5f61231982611221565b915061232483611221565b925082612334576123336122e2565b5b828206905092915050565b5f67ffffffffffffffff82111561235957612358612023565b5b602082029050602081019050919050565b60078110612376575f5ffd5b50565b5f813590506123878161236a565b92915050565b5f61239f61239a8461233f565b612081565b905080838252602082019050602084028301858111156123c2576123c161117d565b5b835b818110156123eb57806123d78882612379565b8452602084019350506020810190506123c4565b5050509392505050565b5f82601f83011261240957612408611175565b5b813561241984826020860161238d565b91505092915050565b5f67ffffffffffffffff82111561243c5761243b612023565b5b602082029050602081019050919050565b5f5ffd5b5f67ffffffffffffffff82111561246b5761246a612023565b5b612474826114a4565b9050602081019050919050565b5f61249361248e84612451565b612081565b9050828152602081018484840111156124af576124ae61244d565b5b6124ba84828561198c565b509392505050565b5f82601f8301126124d6576124d5611175565b5b81356124e6848260208601612481565b91505092915050565b5f6125016124fc84612422565b612081565b905080838252602082019050602084028301858111156125245761252361117d565b5b835b8181101561256b57803567ffffffffffffffff81111561254957612548611175565b5b80860161255689826124c2565b85526020850194505050602081019050612526565b5050509392505050565b5f82601f83011261258957612588611175565b5b81356125998482602086016124ef565b91505092915050565b5f5f604083850312156125b8576125b761116d565b5b5f83013567ffffffffffffffff8111156125d5576125d4611171565b5b6125e1858286016123f5565b925050602083013567ffffffffffffffff81111561260257612601611171565b5b61260e85828601612575565b9150509250929050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52602160045260245ffd5b5f5f6040838503121561265b5761265a61116d565b5b5f61266885828601611a62565b925050602061267985828601611a62565b9150509250929050565b5f61268d826112a7565b9050919050565b61269d81612683565b81146126a7575f5ffd5b50565b5f815190506126b881612694565b92915050565b5f815190506126cc81611252565b92915050565b5f6126e46126df84612451565b612081565b905082815260208101848484011115612700576126ff61244d565b5b61270b848285611496565b509392505050565b5f82601f83011261272757612726611175565b5b81516127378482602086016126d2565b91505092915050565b5f5f5f606084860312156127575761275661116d565b5b5f612764868287016126aa565b9350506020612775868287016126be565b925050604084015167ffffffffffffffff81111561279657612795611171565b5b6127a286828701612713565b9150509250925092565b5f6080820190506127bf5f83018761122a565b6127cc60208301866117bc565b81810360408301526127de81856114b4565b905081810360608301526127f281846114b4565b905095945050505050565b5f602082840312156128125761281161116d565b5b5f82015167ffffffffffffffff81111561282f5761282e611171565b5b61283b84828501612713565b91505092915050565b5f5f6040838503121561285a5761285961116d565b5b5f83015167ffffffffffffffff81111561287757612876611171565b5b61288385828601612713565b925050602061289485828601611a62565b9150509250929050565b5f5f5f606084860312156128b5576128b461116d565b5b5f84015167ffffffffffffffff8111156128d2576128d1611171565b5b6128de86828701612713565b93505060206128ef86828701611a62565b925050604061290086828701611a62565b9150509250925092565b5f5f5f5f608085870312156129225761292161116d565b5b5f85015167ffffffffffffffff81111561293f5761293e611171565b5b61294b87828801612713565b945050602061295c87828801611a62565b935050604061296d87828801611a62565b925050606061297e87828801611a62565b91505092959194509250565b5f5f5f5f5f60a086880312156129a3576129a261116d565b5b5f86015167ffffffffffffffff8111156129c0576129bf611171565b5b6129cc88828901612713565b95505060206129dd88828901611a62565b94505060406129ee88828901611a62565b93505060606129ff88828901611a62565b9250506080612a1088828901611a62565b915050929550929590935056fea264697066735822122049e9130744c49ebfe682e1e4cefc4010fe66682c2e17d1c66b5a395c44576c1d64736f6c634300081c0033
    /// ```
    #[rustfmt::skip]
    #[allow(clippy::all)]
    pub static DEPLOYED_BYTECODE: alloy_sol_types::private::Bytes = alloy_sol_types::private::Bytes::from_static(
        b"`\x80`@R4\x80\x15a\0\x0FW__\xFD[P`\x046\x10a\x01\x14W_5`\xE0\x1C\x80c|\x01Z\x89\x11a\0\xA0W\x80c\xA2{#\xA1\x11a\0oW\x80c\xA2{#\xA1\x14a\x02\xBCW\x80c\xDA2L\x13\x14a\x02\xD8W\x80c\xE0\xF6\xFFC\x14a\x02\xF6W\x80c\xEF\x02DX\x14a\x03\x14W\x80c\xF4\x83> \x14a\x032Wa\x01\x14V[\x80c|\x01Z\x89\x14a\x02FW\x80c\x8B\x97\xC2=\x14a\x02dW\x80c\x8F\x0B\xB7\xCC\x14a\x02\x82W\x80c\x94\xA5\xC2\xE4\x14a\x02\x9EWa\x01\x14V[\x80c3\x1F#\0\x11a\0\xE7W\x80c3\x1F#\0\x14a\x01\xA0W\x80cZ\xFFN.\x14a\x01\xBEW\x80c^8=!\x14a\x01\xDAW\x80c^Q\x0B`\x14a\x02\nW\x80c^\x8B?-\x14a\x02(Wa\x01\x14V[\x80c\x01\x94\xDB\x8E\x14a\x01\x18W\x80c\x08I\xCC\x99\x14a\x014W\x80c\x14.\xDCz\x14a\x01RW\x80c\x1C\x17\x8E\x9C\x14a\x01\x82W[__\xFD[a\x012`\x04\x806\x03\x81\x01\x90a\x01-\x91\x90a\x11\xD6V[a\x03PV[\0[a\x01<a\x03\xA7V[`@Qa\x01I\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x01l`\x04\x806\x03\x81\x01\x90a\x01g\x91\x90a\x12|V[a\x03\xB3V[`@Qa\x01y\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x01\x8Aa\x04\x1FV[`@Qa\x01\x97\x91\x90a\x13!V[`@Q\x80\x91\x03\x90\xF3[a\x01\xA8a\x04CV[`@Qa\x01\xB5\x91\x90a\x13\xF1V[`@Q\x80\x91\x03\x90\xF3[a\x01\xD8`\x04\x806\x03\x81\x01\x90a\x01\xD3\x91\x90a\x12|V[a\x04\x99V[\0[a\x01\xF4`\x04\x806\x03\x81\x01\x90a\x01\xEF\x91\x90a\x12|V[a\x04\xFBV[`@Qa\x02\x01\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x02\x12a\x05\x1BV[`@Qa\x02\x1F\x91\x90a\x14,V[`@Q\x80\x91\x03\x90\xF3[a\x020a\x05.V[`@Qa\x02=\x91\x90a\x14cV[`@Q\x80\x91\x03\x90\xF3[a\x02Na\x05DV[`@Qa\x02[\x91\x90a\x14\xECV[`@Q\x80\x91\x03\x90\xF3[a\x02la\x05\xCFV[`@Qa\x02y\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x02\x9C`\x04\x806\x03\x81\x01\x90a\x02\x97\x91\x90a\x16qV[a\x05\xD5V[\0[a\x02\xA6a\tWV[`@Qa\x02\xB3\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x02\xD6`\x04\x806\x03\x81\x01\x90a\x02\xD1\x91\x90a\x17~V[a\t{V[\0[a\x02\xE0a\n0V[`@Qa\x02\xED\x91\x90a\x17\xCBV[`@Q\x80\x91\x03\x90\xF3[a\x02\xFEa\nUV[`@Qa\x03\x0B\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[a\x03\x1Ca\nyV[`@Qa\x03)\x91\x90a\x14,V[`@Q\x80\x91\x03\x90\xF3[a\x03:a\n~V[`@Qa\x03G\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xF3[\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFT\x80`\x01\x01\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFUPa\x03\xA3\x82\x82a\n\xA6V[PPV[_`\x03\x80T\x90P\x90P\x90V[_`\x03\x80T\x90P\x82\x10a\x03\xFBW`@Q\x7F\x08\xC3y\xA0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\x03\xF2\x90a\x18>V[`@Q\x80\x91\x03\x90\xFD[`\x03\x82\x81T\x81\x10a\x04\x0FWa\x04\x0Ea\x18\\V[[\x90_R` _ \x01T\x90P\x91\x90PV[\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81V[```\x03\x80T\x80` \x02` \x01`@Q\x90\x81\x01`@R\x80\x92\x91\x90\x81\x81R` \x01\x82\x80T\x80\x15a\x04\x8FW` \x02\x82\x01\x91\x90_R` _ \x90[\x81T\x81R` \x01\x90`\x01\x01\x90\x80\x83\x11a\x04{W[PPPPP\x90P\x90V[\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFT\x80`\x01\x01\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFUP`\x03_a\x04\xEF\x91\x90a\x11+V[a\x04\xF8\x81a\x0C\nV[PV[`\x03\x81\x81T\x81\x10a\x05\nW_\x80\xFD[\x90_R` _ \x01_\x91P\x90PT\x81V[`\x01`\x14\x90T\x90a\x01\0\n\x90\x04`\xFF\x16\x81V[`\x01`\x15\x90T\x90a\x01\0\n\x90\x04c\xFF\xFF\xFF\xFF\x16\x81V[_\x80Ta\x05P\x90a\x18\xB6V[\x80`\x1F\x01` \x80\x91\x04\x02` \x01`@Q\x90\x81\x01`@R\x80\x92\x91\x90\x81\x81R` \x01\x82\x80Ta\x05|\x90a\x18\xB6V[\x80\x15a\x05\xC7W\x80`\x1F\x10a\x05\x9EWa\x01\0\x80\x83T\x04\x02\x83R\x91` \x01\x91a\x05\xC7V[\x82\x01\x91\x90_R` _ \x90[\x81T\x81R\x90`\x01\x01\x90` \x01\x80\x83\x11a\x05\xAAW\x82\x90\x03`\x1F\x16\x82\x01\x91[PPPPP\x81V[`\x02T\x81V[\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFT\x80`\x01\x01\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFUPC\x87c\xFF\xFF\xFF\xFF\x16\x10a\x06]W`@Q\x7F%/\x8A\x0E\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[Cc\xFF\xFF\xFF\xFF\x16`\x01`\x15\x90T\x90a\x01\0\n\x90\x04c\xFF\xFF\xFF\xFF\x16\x88a\x06\x82\x91\x90a\x19\x13V[c\xFF\xFF\xFF\xFF\x16\x10\x15a\x06\xC0W`@Q\x7F0\\>\x93\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[a\x06\xC8a\n~V[`\x01\x85a\x06\xD5\x91\x90a\x19JV[\x14a\x07\x0CW`@Q\x7Fsv\xE0\xA2\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[_`\x02\x85\x85\x85\x8A\x8A`@Q` \x01a\x07(\x95\x94\x93\x92\x91\x90a\x19\xC6V[`@Q` \x81\x83\x03\x03\x81R\x90`@R`@Qa\x07D\x91\x90a\x1ALV[` `@Q\x80\x83\x03\x81\x85Z\xFA\x15\x80\x15a\x07_W=__>=_\xFD[PPP`@Q=`\x1F\x19`\x1F\x82\x01\x16\x82\x01\x80`@RP\x81\x01\x90a\x07\x82\x91\x90a\x1AvV[\x90P\x8A\x81\x14a\x07\xBDW`@Q\x7F\x8B\xAAW\x9F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[_\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16cn\xFBF6\x8D\x8D\x8D\x8D\x88`@Q\x86c\xFF\xFF\xFF\xFF\x16`\xE0\x1B\x81R`\x04\x01a\x08\x1F\x95\x94\x93\x92\x91\x90a\x1F\xCCV[_`@Q\x80\x83\x03\x81\x86Z\xFA\x15\x80\x15a\x089W=__>=_\xFD[PPPP`@Q=_\x82>=`\x1F\x19`\x1F\x82\x01\x16\x82\x01\x80`@RP\x81\x01\x90a\x08a\x91\x90a\"%V[P\x90P__\x90P[\x8B\x8B\x90P\x81\x10\x15a\t>W`\x01`\x14\x90T\x90a\x01\0\n\x90\x04`\xFF\x16`\xFF\x16\x82` \x01Q\x82\x81Q\x81\x10a\x08\x9EWa\x08\x9Da\x18\\V[[` \x02` \x01\x01Qa\x08\xB0\x91\x90a\"\x7FV[k\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16`d`\xFF\x16\x83_\x01Q\x83\x81Q\x81\x10a\x08\xD9Wa\x08\xD8a\x18\\V[[` \x02` \x01\x01Qa\x08\xEB\x91\x90a\"\x7FV[k\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x10\x15a\t1W`@Q\x7Fm\x86\x05\xDB\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[\x80\x80`\x01\x01\x91PPa\x08iV[Pa\tI\x88\x88a\rRV[PPPPPPPPPPPPV[\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81V[\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFT\x80`\x01\x01\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFUP`\x03\x80T\x90P\x82\x10a\n\x0BW`@Q\x7F\x08\xC3y\xA0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\n\x02\x90a\x18>V[`@Q\x80\x91\x03\x90\xFD[\x80`\x03\x83\x81T\x81\x10a\n Wa\n\x1Fa\x18\\V[[\x90_R` _ \x01\x81\x90UPPPV[`\x01_\x90T\x90a\x01\0\n\x90\x04s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x81V[\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81V[`d\x81V[_\x7F\xDE\xBF\xDF\xD5\xA5\n\xD1\x17\xC1\x08\x98\xD6\x8B\\\xCF\x08\x93\xC6\xB4\rOD?\x90..vF`\x1B\xDE\xAFT\x90P\x90V[__\x90P_\x83\x83\x90P\x03a\x0B\x03W__\x90P[`\x03\x80T\x90P\x81\x10\x15a\n\xFDW`\x03\x81\x81T\x81\x10a\n\xDAWa\n\xD9a\x18\\V[[\x90_R` _ \x01T\x82a\n\xEE\x91\x90a\x19JV[\x91P\x80\x80`\x01\x01\x91PPa\n\xB9V[Pa\x0B\xC5V[__\x90P[\x83\x83\x90P\x81\x10\x15a\x0B\xC3W`\x03\x80T\x90P\x84\x84\x83\x81\x81\x10a\x0B,Wa\x0B+a\x18\\V[[\x90P` \x02\x015\x10a\x0BsW`@Q\x7F\x08\xC3y\xA0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\x0Bj\x90a\x18>V[`@Q\x80\x91\x03\x90\xFD[`\x03\x84\x84\x83\x81\x81\x10a\x0B\x88Wa\x0B\x87a\x18\\V[[\x90P` \x02\x015\x81T\x81\x10a\x0B\xA0Wa\x0B\x9Fa\x18\\V[[\x90_R` _ \x01T\x82a\x0B\xB4\x91\x90a\x19JV[\x91P\x80\x80`\x01\x01\x91PPa\x0B\x08V[P[\x80`\x02\x81\x90UP\x7F\xFD=\xFB\xB3\xDA\x06\xB2q\x08H\x91le\x86j=\x0E\x05\0G@%y\xA6\xE1qBa\x13|\x19\xC6\x81B`@Qa\x0B\xFD\x92\x91\x90a\"\xBBV[`@Q\x80\x91\x03\x90\xA1PPPV[_\x81\x03a\x0C\x15WB\x90P[_\x81`@Q` \x01a\x0C'\x91\x90a\x129V[`@Q` \x81\x83\x03\x03\x81R\x90`@R\x80Q\x90` \x01 _\x1C\x90P__\x90P[\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81\x10\x15a\x0C\xF6W`\x03\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x83\x83`@Q` \x01a\x0C\xA4\x92\x91\x90a\"\xBBV[`@Q` \x81\x83\x03\x03\x81R\x90`@R\x80Q\x90` \x01 _\x1Ca\x0C\xC6\x91\x90a#\x0FV[\x90\x80`\x01\x81T\x01\x80\x82U\x80\x91PP`\x01\x90\x03\x90_R` _ \x01_\x90\x91\x90\x91\x90\x91PU\x80\x80`\x01\x01\x91PPa\x0CFV[P\x7F\xB6\x0B\x9A\x866\xA9\xD1\xF7ps\x1F\xDCH\x91+\xFD\xAC\xB1\xD8\xE7f\x07\x92\xC9\x1A\x05\x1B\xDD\xF9\xD6-M\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0`@Qa\rF\x91\x90a\x129V[`@Q\x80\x91\x03\x90\xA1PPV[__\x83\x83\x81\x01\x90a\rc\x91\x90a%\xA2V[\x91P\x91Pa\rq\x82\x82a\rwV[PPPPV[\x80Q\x82Q\x14a\r\xB2W`@Q\x7F_o\x13,\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01`@Q\x80\x91\x03\x90\xFD[__\x90P[\x82Q\x81\x10\x15a\x11&W_\x83\x82\x81Q\x81\x10a\r\xD4Wa\r\xD3a\x18\\V[[` \x02` \x01\x01Q\x90P_\x83\x83\x81Q\x81\x10a\r\xF2Wa\r\xF1a\x18\\V[[` \x02` \x01\x01Q\x90P_`\x06\x81\x11\x15a\x0E\x0FWa\x0E\x0Ea&\x18V[[\x82`\x06\x81\x11\x15a\x0E\"Wa\x0E!a&\x18V[[\x03a\x0EKW__\x82\x80` \x01\x90Q\x81\x01\x90a\x0E=\x91\x90a&EV[\x91P\x91P\x80\x82UPPa\x11\x17V[`\x01`\x06\x81\x11\x15a\x0E_Wa\x0E^a&\x18V[[\x82`\x06\x81\x11\x15a\x0ErWa\x0Eqa&\x18V[[\x03a\x0FTW___\x83\x80` \x01\x90Q\x81\x01\x90a\x0E\x8E\x91\x90a'@V[\x92P\x92P\x92P__Z\x90P__\x84Q` \x86\x01\x87\x89\x86\xF1\x91P\x81a\x0FJW_=\x90P_\x81g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x0E\xCBWa\x0E\xCAa #V[[`@Q\x90\x80\x82R\x80`\x1F\x01`\x1F\x19\x16` \x01\x82\x01`@R\x80\x15a\x0E\xFDW\x81` \x01`\x01\x82\x02\x806\x837\x80\x82\x01\x91PP\x90P[P\x90P\x81_` \x83\x01>\x89\x87\x82\x87`@Q\x7FI?\t\xC4\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\x0FA\x94\x93\x92\x91\x90a'\xACV[`@Q\x80\x91\x03\x90\xFD[PPPPPa\x11\x16V[`\x02`\x06\x81\x11\x15a\x0FhWa\x0Fga&\x18V[[\x82`\x06\x81\x11\x15a\x0F{Wa\x0Fza&\x18V[[\x03a\x0F\xA4W_\x81\x80` \x01\x90Q\x81\x01\x90a\x0F\x95\x91\x90a'\xFDV[\x90P\x80Q` \x82\x01\xA0Pa\x11\x15V[`\x03`\x06\x81\x11\x15a\x0F\xB8Wa\x0F\xB7a&\x18V[[\x82`\x06\x81\x11\x15a\x0F\xCBWa\x0F\xCAa&\x18V[[\x03a\x0F\xF9W__\x82\x80` \x01\x90Q\x81\x01\x90a\x0F\xE6\x91\x90a(DV[\x91P\x91P\x80\x82Q` \x84\x01\xA1PPa\x11\x14V[`\x04`\x06\x81\x11\x15a\x10\rWa\x10\x0Ca&\x18V[[\x82`\x06\x81\x11\x15a\x10 Wa\x10\x1Fa&\x18V[[\x03a\x10SW___\x83\x80` \x01\x90Q\x81\x01\x90a\x10<\x91\x90a(\x9EV[\x92P\x92P\x92P\x80\x82\x84Q` \x86\x01\xA2PPPa\x11\x13V[`\x05`\x06\x81\x11\x15a\x10gWa\x10fa&\x18V[[\x82`\x06\x81\x11\x15a\x10zWa\x10ya&\x18V[[\x03a\x10\xB2W____\x84\x80` \x01\x90Q\x81\x01\x90a\x10\x97\x91\x90a)\nV[\x93P\x93P\x93P\x93P\x80\x82\x84\x86Q` \x88\x01\xA3PPPPa\x11\x12V[`\x06\x80\x81\x11\x15a\x10\xC5Wa\x10\xC4a&\x18V[[\x82`\x06\x81\x11\x15a\x10\xD8Wa\x10\xD7a&\x18V[[\x03a\x11\x11W_____\x85\x80` \x01\x90Q\x81\x01\x90a\x10\xF6\x91\x90a)\x8AV[\x94P\x94P\x94P\x94P\x94P\x80\x82\x84\x86\x88Q` \x8A\x01\xA4PPPPP[[[[[[[PP\x80\x80`\x01\x01\x91PPa\r\xB7V[PPPV[P\x80T_\x82U\x90_R` _ \x90\x81\x01\x90a\x11F\x91\x90a\x11IV[PV[[\x80\x82\x11\x15a\x11`W_\x81_\x90UP`\x01\x01a\x11JV[P\x90V[_`@Q\x90P\x90V[__\xFD[__\xFD[__\xFD[__\xFD[__\xFD[__\x83`\x1F\x84\x01\x12a\x11\x96Wa\x11\x95a\x11uV[[\x825\x90Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x11\xB3Wa\x11\xB2a\x11yV[[` \x83\x01\x91P\x83` \x82\x02\x83\x01\x11\x15a\x11\xCFWa\x11\xCEa\x11}V[[\x92P\x92\x90PV[__` \x83\x85\x03\x12\x15a\x11\xECWa\x11\xEBa\x11mV[[_\x83\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x12\tWa\x12\x08a\x11qV[[a\x12\x15\x85\x82\x86\x01a\x11\x81V[\x92P\x92PP\x92P\x92\x90PV[_\x81\x90P\x91\x90PV[a\x123\x81a\x12!V[\x82RPPV[_` \x82\x01\x90Pa\x12L_\x83\x01\x84a\x12*V[\x92\x91PPV[a\x12[\x81a\x12!V[\x81\x14a\x12eW__\xFD[PV[_\x815\x90Pa\x12v\x81a\x12RV[\x92\x91PPV[_` \x82\x84\x03\x12\x15a\x12\x91Wa\x12\x90a\x11mV[[_a\x12\x9E\x84\x82\x85\x01a\x12hV[\x91PP\x92\x91PPV[_s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[_\x81\x90P\x91\x90PV[_a\x12\xE9a\x12\xE4a\x12\xDF\x84a\x12\xA7V[a\x12\xC6V[a\x12\xA7V[\x90P\x91\x90PV[_a\x12\xFA\x82a\x12\xCFV[\x90P\x91\x90PV[_a\x13\x0B\x82a\x12\xF0V[\x90P\x91\x90PV[a\x13\x1B\x81a\x13\x01V[\x82RPPV[_` \x82\x01\x90Pa\x134_\x83\x01\x84a\x13\x12V[\x92\x91PPV[_\x81Q\x90P\x91\x90PV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[_\x81\x90P` \x82\x01\x90P\x91\x90PV[a\x13l\x81a\x12!V[\x82RPPV[_a\x13}\x83\x83a\x13cV[` \x83\x01\x90P\x92\x91PPV[_` \x82\x01\x90P\x91\x90PV[_a\x13\x9F\x82a\x13:V[a\x13\xA9\x81\x85a\x13DV[\x93Pa\x13\xB4\x83a\x13TV[\x80_[\x83\x81\x10\x15a\x13\xE4W\x81Qa\x13\xCB\x88\x82a\x13rV[\x97Pa\x13\xD6\x83a\x13\x89V[\x92PP`\x01\x81\x01\x90Pa\x13\xB7V[P\x85\x93PPPP\x92\x91PPV[_` \x82\x01\x90P\x81\x81\x03_\x83\x01Ra\x14\t\x81\x84a\x13\x95V[\x90P\x92\x91PPV[_`\xFF\x82\x16\x90P\x91\x90PV[a\x14&\x81a\x14\x11V[\x82RPPV[_` \x82\x01\x90Pa\x14?_\x83\x01\x84a\x14\x1DV[\x92\x91PPV[_c\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[a\x14]\x81a\x14EV[\x82RPPV[_` \x82\x01\x90Pa\x14v_\x83\x01\x84a\x14TV[\x92\x91PPV[_\x81Q\x90P\x91\x90PV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[\x82\x81\x83^_\x83\x83\x01RPPPV[_`\x1F\x19`\x1F\x83\x01\x16\x90P\x91\x90PV[_a\x14\xBE\x82a\x14|V[a\x14\xC8\x81\x85a\x14\x86V[\x93Pa\x14\xD8\x81\x85` \x86\x01a\x14\x96V[a\x14\xE1\x81a\x14\xA4V[\x84\x01\x91PP\x92\x91PPV[_` \x82\x01\x90P\x81\x81\x03_\x83\x01Ra\x15\x04\x81\x84a\x14\xB4V[\x90P\x92\x91PPV[_\x81\x90P\x91\x90PV[a\x15\x1E\x81a\x15\x0CV[\x81\x14a\x15(W__\xFD[PV[_\x815\x90Pa\x159\x81a\x15\x15V[\x92\x91PPV[__\x83`\x1F\x84\x01\x12a\x15TWa\x15Sa\x11uV[[\x825\x90Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x15qWa\x15pa\x11yV[[` \x83\x01\x91P\x83`\x01\x82\x02\x83\x01\x11\x15a\x15\x8DWa\x15\x8Ca\x11}V[[\x92P\x92\x90PV[a\x15\x9D\x81a\x14EV[\x81\x14a\x15\xA7W__\xFD[PV[_\x815\x90Pa\x15\xB8\x81a\x15\x94V[\x92\x91PPV[_a\x15\xC8\x82a\x12\xA7V[\x90P\x91\x90PV[a\x15\xD8\x81a\x15\xBEV[\x81\x14a\x15\xE2W__\xFD[PV[_\x815\x90Pa\x15\xF3\x81a\x15\xCFV[\x92\x91PPV[_\x7F\xFF\xFF\xFF\xFF\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x82\x16\x90P\x91\x90PV[a\x16-\x81a\x15\xF9V[\x81\x14a\x167W__\xFD[PV[_\x815\x90Pa\x16H\x81a\x16$V[\x92\x91PPV[__\xFD[_a\x01\x80\x82\x84\x03\x12\x15a\x16hWa\x16ga\x16NV[[\x81\x90P\x92\x91PPV[__________a\x01\0\x8B\x8D\x03\x12\x15a\x16\x90Wa\x16\x8Fa\x11mV[[_a\x16\x9D\x8D\x82\x8E\x01a\x15+V[\x9APP` \x8B\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x16\xBEWa\x16\xBDa\x11qV[[a\x16\xCA\x8D\x82\x8E\x01a\x15?V[\x99P\x99PP`@a\x16\xDD\x8D\x82\x8E\x01a\x15\xAAV[\x97PP``\x8B\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x16\xFEWa\x16\xFDa\x11qV[[a\x17\n\x8D\x82\x8E\x01a\x15?V[\x96P\x96PP`\x80a\x17\x1D\x8D\x82\x8E\x01a\x12hV[\x94PP`\xA0a\x17.\x8D\x82\x8E\x01a\x15\xE5V[\x93PP`\xC0a\x17?\x8D\x82\x8E\x01a\x16:V[\x92PP`\xE0\x8B\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x17`Wa\x17_a\x11qV[[a\x17l\x8D\x82\x8E\x01a\x16RV[\x91PP\x92\x95\x98\x9B\x91\x94\x97\x9AP\x92\x95\x98PV[__`@\x83\x85\x03\x12\x15a\x17\x94Wa\x17\x93a\x11mV[[_a\x17\xA1\x85\x82\x86\x01a\x12hV[\x92PP` a\x17\xB2\x85\x82\x86\x01a\x12hV[\x91PP\x92P\x92\x90PV[a\x17\xC5\x81a\x15\xBEV[\x82RPPV[_` \x82\x01\x90Pa\x17\xDE_\x83\x01\x84a\x17\xBCV[\x92\x91PPV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[\x7FIndex out of bounds\0\0\0\0\0\0\0\0\0\0\0\0\0_\x82\x01RPV[_a\x18(`\x13\x83a\x17\xE4V[\x91Pa\x183\x82a\x17\xF4V[` \x82\x01\x90P\x91\x90PV[_` \x82\x01\x90P\x81\x81\x03_\x83\x01Ra\x18U\x81a\x18\x1CV[\x90P\x91\x90PV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`2`\x04R`$_\xFD[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`\"`\x04R`$_\xFD[_`\x02\x82\x04\x90P`\x01\x82\x16\x80a\x18\xCDW`\x7F\x82\x16\x91P[` \x82\x10\x81\x03a\x18\xE0Wa\x18\xDFa\x18\x89V[[P\x91\x90PV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`\x11`\x04R`$_\xFD[_a\x19\x1D\x82a\x14EV[\x91Pa\x19(\x83a\x14EV[\x92P\x82\x82\x01\x90Pc\xFF\xFF\xFF\xFF\x81\x11\x15a\x19DWa\x19Ca\x18\xE6V[[\x92\x91PPV[_a\x19T\x82a\x12!V[\x91Pa\x19_\x83a\x12!V[\x92P\x82\x82\x01\x90P\x80\x82\x11\x15a\x19wWa\x19va\x18\xE6V[[\x92\x91PPV[a\x19\x86\x81a\x15\xF9V[\x82RPPV[\x82\x81\x837_\x83\x83\x01RPPPV[_a\x19\xA5\x83\x85a\x14\x86V[\x93Pa\x19\xB2\x83\x85\x84a\x19\x8CV[a\x19\xBB\x83a\x14\xA4V[\x84\x01\x90P\x93\x92PPPV[_`\x80\x82\x01\x90Pa\x19\xD9_\x83\x01\x88a\x12*V[a\x19\xE6` \x83\x01\x87a\x17\xBCV[a\x19\xF3`@\x83\x01\x86a\x19}V[\x81\x81\x03``\x83\x01Ra\x1A\x06\x81\x84\x86a\x19\x9AV[\x90P\x96\x95PPPPPPV[_\x81\x90P\x92\x91PPV[_a\x1A&\x82a\x14|V[a\x1A0\x81\x85a\x1A\x12V[\x93Pa\x1A@\x81\x85` \x86\x01a\x14\x96V[\x80\x84\x01\x91PP\x92\x91PPV[_a\x1AW\x82\x84a\x1A\x1CV[\x91P\x81\x90P\x92\x91PPV[_\x81Q\x90Pa\x1Ap\x81a\x15\x15V[\x92\x91PPV[_` \x82\x84\x03\x12\x15a\x1A\x8BWa\x1A\x8Aa\x11mV[[_a\x1A\x98\x84\x82\x85\x01a\x1AbV[\x91PP\x92\x91PPV[a\x1A\xAA\x81a\x15\x0CV[\x82RPPV[__\xFD[__\xFD[__\xFD[__\x835`\x01` \x03\x846\x03\x03\x81\x12a\x1A\xD8Wa\x1A\xD7a\x1A\xB8V[[\x83\x81\x01\x92P\x825\x91P` \x83\x01\x92Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a\x1B\0Wa\x1A\xFFa\x1A\xB0V[[` \x82\x026\x03\x83\x13\x15a\x1B\x16Wa\x1B\x15a\x1A\xB4V[[P\x92P\x92\x90PV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[_\x81\x90P\x91\x90PV[a\x1B@\x81a\x14EV[\x82RPPV[_a\x1BQ\x83\x83a\x1B7V[` \x83\x01\x90P\x92\x91PPV[_a\x1Bk` \x84\x01\x84a\x15\xAAV[\x90P\x92\x91PPV[_` \x82\x01\x90P\x91\x90PV[_a\x1B\x8A\x83\x85a\x1B\x1EV[\x93Pa\x1B\x95\x82a\x1B.V[\x80_[\x85\x81\x10\x15a\x1B\xCDWa\x1B\xAA\x82\x84a\x1B]V[a\x1B\xB4\x88\x82a\x1BFV[\x97Pa\x1B\xBF\x83a\x1BsV[\x92PP`\x01\x81\x01\x90Pa\x1B\x98V[P\x85\x92PPP\x93\x92PPPV[__\x835`\x01` \x03\x846\x03\x03\x81\x12a\x1B\xF6Wa\x1B\xF5a\x1A\xB8V[[\x83\x81\x01\x92P\x825\x91P` \x83\x01\x92Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a\x1C\x1EWa\x1C\x1Da\x1A\xB0V[[`@\x82\x026\x03\x83\x13\x15a\x1C4Wa\x1C3a\x1A\xB4V[[P\x92P\x92\x90PV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[_\x81\x90P\x91\x90PV[_a\x1Cc` \x84\x01\x84a\x12hV[\x90P\x92\x91PPV[`@\x82\x01a\x1C{_\x83\x01\x83a\x1CUV[a\x1C\x87_\x85\x01\x82a\x13cV[Pa\x1C\x95` \x83\x01\x83a\x1CUV[a\x1C\xA2` \x85\x01\x82a\x13cV[PPPPV[_a\x1C\xB3\x83\x83a\x1CkV[`@\x83\x01\x90P\x92\x91PPV[_\x82\x90P\x92\x91PPV[_`@\x82\x01\x90P\x91\x90PV[_a\x1C\xE0\x83\x85a\x1C<V[\x93Pa\x1C\xEB\x82a\x1CLV[\x80_[\x85\x81\x10\x15a\x1D#Wa\x1D\0\x82\x84a\x1C\xBFV[a\x1D\n\x88\x82a\x1C\xA8V[\x97Pa\x1D\x15\x83a\x1C\xC9V[\x92PP`\x01\x81\x01\x90Pa\x1C\xEEV[P\x85\x92PPP\x93\x92PPPV[_\x82\x90P\x92\x91PPV[_\x82\x90P\x92\x91PPV[\x82\x81\x837PPPV[a\x1DY`@\x83\x83a\x1DDV[PPV[`\x80\x82\x01a\x1Dm_\x83\x01\x83a\x1D:V[a\x1Dy_\x85\x01\x82a\x1DMV[Pa\x1D\x87`@\x83\x01\x83a\x1D:V[a\x1D\x94`@\x85\x01\x82a\x1DMV[PPPPV[__\x835`\x01` \x03\x846\x03\x03\x81\x12a\x1D\xB6Wa\x1D\xB5a\x1A\xB8V[[\x83\x81\x01\x92P\x825\x91P` \x83\x01\x92Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a\x1D\xDEWa\x1D\xDDa\x1A\xB0V[[` \x82\x026\x03\x83\x13\x15a\x1D\xF4Wa\x1D\xF3a\x1A\xB4V[[P\x92P\x92\x90PV[_\x82\x82R` \x82\x01\x90P\x92\x91PPV[_\x81\x90P\x91\x90PV[_a\x1E!\x84\x84\x84a\x1B\x7FV[\x90P\x93\x92PPPV[_` \x82\x01\x90P\x91\x90PV[_a\x1EA\x83\x85a\x1D\xFCV[\x93P\x83` \x84\x02\x85\x01a\x1ES\x84a\x1E\x0CV[\x80_[\x87\x81\x10\x15a\x1E\x98W\x84\x84\x03\x89Ra\x1Em\x82\x84a\x1A\xBCV[a\x1Ex\x86\x82\x84a\x1E\x15V[\x95Pa\x1E\x83\x84a\x1E*V[\x93P` \x8B\x01\x9APPP`\x01\x81\x01\x90Pa\x1EVV[P\x82\x97P\x87\x94PPPPP\x93\x92PPPV[_a\x01\x80\x83\x01a\x1E\xBC_\x84\x01\x84a\x1A\xBCV[\x85\x83\x03_\x87\x01Ra\x1E\xCE\x83\x82\x84a\x1B\x7FV[\x92PPPa\x1E\xDF` \x84\x01\x84a\x1B\xDAV[\x85\x83\x03` \x87\x01Ra\x1E\xF2\x83\x82\x84a\x1C\xD5V[\x92PPPa\x1F\x03`@\x84\x01\x84a\x1B\xDAV[\x85\x83\x03`@\x87\x01Ra\x1F\x16\x83\x82\x84a\x1C\xD5V[\x92PPPa\x1F'``\x84\x01\x84a\x1D0V[a\x1F4``\x86\x01\x82a\x1D]V[Pa\x1FB`\xE0\x84\x01\x84a\x1C\xBFV[a\x1FO`\xE0\x86\x01\x82a\x1CkV[Pa\x1F^a\x01 \x84\x01\x84a\x1A\xBCV[\x85\x83\x03a\x01 \x87\x01Ra\x1Fr\x83\x82\x84a\x1B\x7FV[\x92PPPa\x1F\x84a\x01@\x84\x01\x84a\x1A\xBCV[\x85\x83\x03a\x01@\x87\x01Ra\x1F\x98\x83\x82\x84a\x1B\x7FV[\x92PPPa\x1F\xAAa\x01`\x84\x01\x84a\x1D\x9AV[\x85\x83\x03a\x01`\x87\x01Ra\x1F\xBE\x83\x82\x84a\x1E6V[\x92PPP\x80\x91PP\x92\x91PPV[_`\x80\x82\x01\x90Pa\x1F\xDF_\x83\x01\x88a\x1A\xA1V[\x81\x81\x03` \x83\x01Ra\x1F\xF2\x81\x86\x88a\x19\x9AV[\x90Pa \x01`@\x83\x01\x85a\x14TV[\x81\x81\x03``\x83\x01Ra \x13\x81\x84a\x1E\xAAV[\x90P\x96\x95PPPPPPV[__\xFD[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`A`\x04R`$_\xFD[a Y\x82a\x14\xA4V[\x81\x01\x81\x81\x10g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x17\x15a xWa wa #V[[\x80`@RPPPV[_a \x8Aa\x11dV[\x90Pa \x96\x82\x82a PV[\x91\x90PV[__\xFD[_g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a \xB9Wa \xB8a #V[[` \x82\x02\x90P` \x81\x01\x90P\x91\x90PV[_k\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[a \xEA\x81a \xCAV[\x81\x14a \xF4W__\xFD[PV[_\x81Q\x90Pa!\x05\x81a \xE1V[\x92\x91PPV[_a!\x1Da!\x18\x84a \x9FV[a \x81V[\x90P\x80\x83\x82R` \x82\x01\x90P` \x84\x02\x83\x01\x85\x81\x11\x15a!@Wa!?a\x11}V[[\x83[\x81\x81\x10\x15a!iW\x80a!U\x88\x82a \xF7V[\x84R` \x84\x01\x93PP` \x81\x01\x90Pa!BV[PPP\x93\x92PPPV[_\x82`\x1F\x83\x01\x12a!\x87Wa!\x86a\x11uV[[\x81Qa!\x97\x84\x82` \x86\x01a!\x0BV[\x91PP\x92\x91PPV[_`@\x82\x84\x03\x12\x15a!\xB5Wa!\xB4a \x1FV[[a!\xBF`@a \x81V[\x90P_\x82\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a!\xDEWa!\xDDa \x9BV[[a!\xEA\x84\x82\x85\x01a!sV[_\x83\x01RP` \x82\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\"\rWa\"\x0Ca \x9BV[[a\"\x19\x84\x82\x85\x01a!sV[` \x83\x01RP\x92\x91PPV[__`@\x83\x85\x03\x12\x15a\";Wa\":a\x11mV[[_\x83\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\"XWa\"Wa\x11qV[[a\"d\x85\x82\x86\x01a!\xA0V[\x92PP` a\"u\x85\x82\x86\x01a\x1AbV[\x91PP\x92P\x92\x90PV[_a\"\x89\x82a \xCAV[\x91Pa\"\x94\x83a \xCAV[\x92P\x82\x82\x02a\"\xA2\x81a \xCAV[\x91P\x80\x82\x14a\"\xB4Wa\"\xB3a\x18\xE6V[[P\x92\x91PPV[_`@\x82\x01\x90Pa\"\xCE_\x83\x01\x85a\x12*V[a\"\xDB` \x83\x01\x84a\x12*V[\x93\x92PPPV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`\x12`\x04R`$_\xFD[_a#\x19\x82a\x12!V[\x91Pa#$\x83a\x12!V[\x92P\x82a#4Wa#3a\"\xE2V[[\x82\x82\x06\x90P\x92\x91PPV[_g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a#YWa#Xa #V[[` \x82\x02\x90P` \x81\x01\x90P\x91\x90PV[`\x07\x81\x10a#vW__\xFD[PV[_\x815\x90Pa#\x87\x81a#jV[\x92\x91PPV[_a#\x9Fa#\x9A\x84a#?V[a \x81V[\x90P\x80\x83\x82R` \x82\x01\x90P` \x84\x02\x83\x01\x85\x81\x11\x15a#\xC2Wa#\xC1a\x11}V[[\x83[\x81\x81\x10\x15a#\xEBW\x80a#\xD7\x88\x82a#yV[\x84R` \x84\x01\x93PP` \x81\x01\x90Pa#\xC4V[PPP\x93\x92PPPV[_\x82`\x1F\x83\x01\x12a$\tWa$\x08a\x11uV[[\x815a$\x19\x84\x82` \x86\x01a#\x8DV[\x91PP\x92\x91PPV[_g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a$<Wa$;a #V[[` \x82\x02\x90P` \x81\x01\x90P\x91\x90PV[__\xFD[_g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a$kWa$ja #V[[a$t\x82a\x14\xA4V[\x90P` \x81\x01\x90P\x91\x90PV[_a$\x93a$\x8E\x84a$QV[a \x81V[\x90P\x82\x81R` \x81\x01\x84\x84\x84\x01\x11\x15a$\xAFWa$\xAEa$MV[[a$\xBA\x84\x82\x85a\x19\x8CV[P\x93\x92PPPV[_\x82`\x1F\x83\x01\x12a$\xD6Wa$\xD5a\x11uV[[\x815a$\xE6\x84\x82` \x86\x01a$\x81V[\x91PP\x92\x91PPV[_a%\x01a$\xFC\x84a$\"V[a \x81V[\x90P\x80\x83\x82R` \x82\x01\x90P` \x84\x02\x83\x01\x85\x81\x11\x15a%$Wa%#a\x11}V[[\x83[\x81\x81\x10\x15a%kW\x805g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a%IWa%Ha\x11uV[[\x80\x86\x01a%V\x89\x82a$\xC2V[\x85R` \x85\x01\x94PPP` \x81\x01\x90Pa%&V[PPP\x93\x92PPPV[_\x82`\x1F\x83\x01\x12a%\x89Wa%\x88a\x11uV[[\x815a%\x99\x84\x82` \x86\x01a$\xEFV[\x91PP\x92\x91PPV[__`@\x83\x85\x03\x12\x15a%\xB8Wa%\xB7a\x11mV[[_\x83\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a%\xD5Wa%\xD4a\x11qV[[a%\xE1\x85\x82\x86\x01a#\xF5V[\x92PP` \x83\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a&\x02Wa&\x01a\x11qV[[a&\x0E\x85\x82\x86\x01a%uV[\x91PP\x92P\x92\x90PV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0_R`!`\x04R`$_\xFD[__`@\x83\x85\x03\x12\x15a&[Wa&Za\x11mV[[_a&h\x85\x82\x86\x01a\x1AbV[\x92PP` a&y\x85\x82\x86\x01a\x1AbV[\x91PP\x92P\x92\x90PV[_a&\x8D\x82a\x12\xA7V[\x90P\x91\x90PV[a&\x9D\x81a&\x83V[\x81\x14a&\xA7W__\xFD[PV[_\x81Q\x90Pa&\xB8\x81a&\x94V[\x92\x91PPV[_\x81Q\x90Pa&\xCC\x81a\x12RV[\x92\x91PPV[_a&\xE4a&\xDF\x84a$QV[a \x81V[\x90P\x82\x81R` \x81\x01\x84\x84\x84\x01\x11\x15a'\0Wa&\xFFa$MV[[a'\x0B\x84\x82\x85a\x14\x96V[P\x93\x92PPPV[_\x82`\x1F\x83\x01\x12a''Wa'&a\x11uV[[\x81Qa'7\x84\x82` \x86\x01a&\xD2V[\x91PP\x92\x91PPV[___``\x84\x86\x03\x12\x15a'WWa'Va\x11mV[[_a'd\x86\x82\x87\x01a&\xAAV[\x93PP` a'u\x86\x82\x87\x01a&\xBEV[\x92PP`@\x84\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a'\x96Wa'\x95a\x11qV[[a'\xA2\x86\x82\x87\x01a'\x13V[\x91PP\x92P\x92P\x92V[_`\x80\x82\x01\x90Pa'\xBF_\x83\x01\x87a\x12*V[a'\xCC` \x83\x01\x86a\x17\xBCV[\x81\x81\x03`@\x83\x01Ra'\xDE\x81\x85a\x14\xB4V[\x90P\x81\x81\x03``\x83\x01Ra'\xF2\x81\x84a\x14\xB4V[\x90P\x95\x94PPPPPV[_` \x82\x84\x03\x12\x15a(\x12Wa(\x11a\x11mV[[_\x82\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a(/Wa(.a\x11qV[[a(;\x84\x82\x85\x01a'\x13V[\x91PP\x92\x91PPV[__`@\x83\x85\x03\x12\x15a(ZWa(Ya\x11mV[[_\x83\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a(wWa(va\x11qV[[a(\x83\x85\x82\x86\x01a'\x13V[\x92PP` a(\x94\x85\x82\x86\x01a\x1AbV[\x91PP\x92P\x92\x90PV[___``\x84\x86\x03\x12\x15a(\xB5Wa(\xB4a\x11mV[[_\x84\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a(\xD2Wa(\xD1a\x11qV[[a(\xDE\x86\x82\x87\x01a'\x13V[\x93PP` a(\xEF\x86\x82\x87\x01a\x1AbV[\x92PP`@a)\0\x86\x82\x87\x01a\x1AbV[\x91PP\x92P\x92P\x92V[____`\x80\x85\x87\x03\x12\x15a)\"Wa)!a\x11mV[[_\x85\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a)?Wa)>a\x11qV[[a)K\x87\x82\x88\x01a'\x13V[\x94PP` a)\\\x87\x82\x88\x01a\x1AbV[\x93PP`@a)m\x87\x82\x88\x01a\x1AbV[\x92PP``a)~\x87\x82\x88\x01a\x1AbV[\x91PP\x92\x95\x91\x94P\x92PV[_____`\xA0\x86\x88\x03\x12\x15a)\xA3Wa)\xA2a\x11mV[[_\x86\x01Qg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a)\xC0Wa)\xBFa\x11qV[[a)\xCC\x88\x82\x89\x01a'\x13V[\x95PP` a)\xDD\x88\x82\x89\x01a\x1AbV[\x94PP`@a)\xEE\x88\x82\x89\x01a\x1AbV[\x93PP``a)\xFF\x88\x82\x89\x01a\x1AbV[\x92PP`\x80a*\x10\x88\x82\x89\x01a\x1AbV[\x91PP\x92\x95P\x92\x95\x90\x93PV\xFE\xA2dipfsX\"\x12 I\xE9\x13\x07D\xC4\x9E\xBF\xE6\x82\xE1\xE4\xCE\xFC@\x10\xFEfh,.\x17\xD1\xC6kZ9\\DWl\x1DdsolcC\0\x08\x1C\x003",
    );
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Custom error with signature `FutureBlockNumber()` and selector `0x252f8a0e`.
```solidity
error FutureBlockNumber();
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct FutureBlockNumber {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = ();
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = ();
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<FutureBlockNumber> for UnderlyingRustTuple<'_> {
            fn from(value: FutureBlockNumber) -> Self {
                ()
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for FutureBlockNumber {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {}
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolError for FutureBlockNumber {
            type Parameters<'a> = UnderlyingSolTuple<'a>;
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "FutureBlockNumber()";
            const SELECTOR: [u8; 4] = [37u8, 47u8, 138u8, 14u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Custom error with signature `InsufficientQuorumThreshold()` and selector `0x6d8605db`.
```solidity
error InsufficientQuorumThreshold();
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct InsufficientQuorumThreshold {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = ();
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = ();
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<InsufficientQuorumThreshold>
        for UnderlyingRustTuple<'_> {
            fn from(value: InsufficientQuorumThreshold) -> Self {
                ()
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>>
        for InsufficientQuorumThreshold {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {}
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolError for InsufficientQuorumThreshold {
            type Parameters<'a> = UnderlyingSolTuple<'a>;
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "InsufficientQuorumThreshold()";
            const SELECTOR: [u8; 4] = [109u8, 134u8, 5u8, 219u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Custom error with signature `InvalidArguments()` and selector `0x5f6f132c`.
```solidity
error InvalidArguments();
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct InvalidArguments {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = ();
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = ();
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<InvalidArguments> for UnderlyingRustTuple<'_> {
            fn from(value: InvalidArguments) -> Self {
                ()
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for InvalidArguments {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {}
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolError for InvalidArguments {
            type Parameters<'a> = UnderlyingSolTuple<'a>;
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "InvalidArguments()";
            const SELECTOR: [u8; 4] = [95u8, 111u8, 19u8, 44u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Custom error with signature `InvalidConfiguration()` and selector `0xc52a9bd3`.
```solidity
error InvalidConfiguration();
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct InvalidConfiguration {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = ();
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = ();
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<InvalidConfiguration> for UnderlyingRustTuple<'_> {
            fn from(value: InvalidConfiguration) -> Self {
                ()
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for InvalidConfiguration {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {}
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolError for InvalidConfiguration {
            type Parameters<'a> = UnderlyingSolTuple<'a>;
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "InvalidConfiguration()";
            const SELECTOR: [u8; 4] = [197u8, 42u8, 155u8, 211u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Custom error with signature `InvalidOperation()` and selector `0x398d4d32`.
```solidity
error InvalidOperation();
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct InvalidOperation {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = ();
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = ();
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<InvalidOperation> for UnderlyingRustTuple<'_> {
            fn from(value: InvalidOperation) -> Self {
                ()
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for InvalidOperation {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {}
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolError for InvalidOperation {
            type Parameters<'a> = UnderlyingSolTuple<'a>;
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "InvalidOperation()";
            const SELECTOR: [u8; 4] = [57u8, 141u8, 77u8, 50u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Custom error with signature `InvalidSignature()` and selector `0x8baa579f`.
```solidity
error InvalidSignature();
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct InvalidSignature {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = ();
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = ();
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<InvalidSignature> for UnderlyingRustTuple<'_> {
            fn from(value: InvalidSignature) -> Self {
                ()
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for InvalidSignature {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {}
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolError for InvalidSignature {
            type Parameters<'a> = UnderlyingSolTuple<'a>;
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "InvalidSignature()";
            const SELECTOR: [u8; 4] = [139u8, 170u8, 87u8, 159u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Custom error with signature `InvalidStorageUpdates()` and selector `0xfbbb7b2b`.
```solidity
error InvalidStorageUpdates();
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct InvalidStorageUpdates {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = ();
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = ();
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<InvalidStorageUpdates> for UnderlyingRustTuple<'_> {
            fn from(value: InvalidStorageUpdates) -> Self {
                ()
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for InvalidStorageUpdates {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {}
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolError for InvalidStorageUpdates {
            type Parameters<'a> = UnderlyingSolTuple<'a>;
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "InvalidStorageUpdates()";
            const SELECTOR: [u8; 4] = [251u8, 187u8, 123u8, 43u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Custom error with signature `InvalidTransitionIndex()` and selector `0x7376e0a2`.
```solidity
error InvalidTransitionIndex();
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct InvalidTransitionIndex {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = ();
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = ();
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<InvalidTransitionIndex> for UnderlyingRustTuple<'_> {
            fn from(value: InvalidTransitionIndex) -> Self {
                ()
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for InvalidTransitionIndex {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {}
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolError for InvalidTransitionIndex {
            type Parameters<'a> = UnderlyingSolTuple<'a>;
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "InvalidTransitionIndex()";
            const SELECTOR: [u8; 4] = [115u8, 118u8, 224u8, 162u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Custom error with signature `RevertingContext(uint256,address,bytes,bytes)` and selector `0x493f09c4`.
```solidity
error RevertingContext(uint256 index, address target, bytes revertData, bytes callargs);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct RevertingContext {
        #[allow(missing_docs)]
        pub index: alloy::sol_types::private::primitives::aliases::U256,
        #[allow(missing_docs)]
        pub target: alloy::sol_types::private::Address,
        #[allow(missing_docs)]
        pub revertData: alloy::sol_types::private::Bytes,
        #[allow(missing_docs)]
        pub callargs: alloy::sol_types::private::Bytes,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = (
            alloy::sol_types::sol_data::Uint<256>,
            alloy::sol_types::sol_data::Address,
            alloy::sol_types::sol_data::Bytes,
            alloy::sol_types::sol_data::Bytes,
        );
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = (
            alloy::sol_types::private::primitives::aliases::U256,
            alloy::sol_types::private::Address,
            alloy::sol_types::private::Bytes,
            alloy::sol_types::private::Bytes,
        );
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<RevertingContext> for UnderlyingRustTuple<'_> {
            fn from(value: RevertingContext) -> Self {
                (value.index, value.target, value.revertData, value.callargs)
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for RevertingContext {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {
                    index: tuple.0,
                    target: tuple.1,
                    revertData: tuple.2,
                    callargs: tuple.3,
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolError for RevertingContext {
            type Parameters<'a> = UnderlyingSolTuple<'a>;
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "RevertingContext(uint256,address,bytes,bytes)";
            const SELECTOR: [u8; 4] = [73u8, 63u8, 9u8, 196u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                (
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self.index),
                    <alloy::sol_types::sol_data::Address as alloy_sol_types::SolType>::tokenize(
                        &self.target,
                    ),
                    <alloy::sol_types::sol_data::Bytes as alloy_sol_types::SolType>::tokenize(
                        &self.revertData,
                    ),
                    <alloy::sol_types::sol_data::Bytes as alloy_sol_types::SolType>::tokenize(
                        &self.callargs,
                    ),
                )
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Custom error with signature `StaleBlockNumber()` and selector `0x305c3e93`.
```solidity
error StaleBlockNumber();
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct StaleBlockNumber {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[doc(hidden)]
        type UnderlyingSolTuple<'a> = ();
        #[doc(hidden)]
        type UnderlyingRustTuple<'a> = ();
        #[cfg(test)]
        #[allow(dead_code, unreachable_patterns)]
        fn _type_assertion(
            _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
        ) {
            match _t {
                alloy_sol_types::private::AssertTypeEq::<
                    <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                >(_) => {}
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<StaleBlockNumber> for UnderlyingRustTuple<'_> {
            fn from(value: StaleBlockNumber) -> Self {
                ()
            }
        }
        #[automatically_derived]
        #[doc(hidden)]
        impl ::core::convert::From<UnderlyingRustTuple<'_>> for StaleBlockNumber {
            fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                Self {}
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolError for StaleBlockNumber {
            type Parameters<'a> = UnderlyingSolTuple<'a>;
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "StaleBlockNumber()";
            const SELECTOR: [u8; 4] = [48u8, 92u8, 62u8, 147u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Event with signature `ArrayInitialized(uint256)` and selector `0xb60b9a8636a9d1f770731fdc48912bfdacb1d8e7660792c91a051bddf9d62d4d`.
```solidity
event ArrayInitialized(uint256 size);
```*/
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    #[derive(Clone)]
    pub struct ArrayInitialized {
        #[allow(missing_docs)]
        pub size: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[automatically_derived]
        impl alloy_sol_types::SolEvent for ArrayInitialized {
            type DataTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type DataToken<'a> = <Self::DataTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type TopicList = (alloy_sol_types::sol_data::FixedBytes<32>,);
            const SIGNATURE: &'static str = "ArrayInitialized(uint256)";
            const SIGNATURE_HASH: alloy_sol_types::private::B256 = alloy_sol_types::private::B256::new([
                182u8, 11u8, 154u8, 134u8, 54u8, 169u8, 209u8, 247u8, 112u8, 115u8, 31u8,
                220u8, 72u8, 145u8, 43u8, 253u8, 172u8, 177u8, 216u8, 231u8, 102u8, 7u8,
                146u8, 201u8, 26u8, 5u8, 27u8, 221u8, 249u8, 214u8, 45u8, 77u8,
            ]);
            const ANONYMOUS: bool = false;
            #[allow(unused_variables)]
            #[inline]
            fn new(
                topics: <Self::TopicList as alloy_sol_types::SolType>::RustType,
                data: <Self::DataTuple<'_> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                Self { size: data.0 }
            }
            #[inline]
            fn check_signature(
                topics: &<Self::TopicList as alloy_sol_types::SolType>::RustType,
            ) -> alloy_sol_types::Result<()> {
                if topics.0 != Self::SIGNATURE_HASH {
                    return Err(
                        alloy_sol_types::Error::invalid_event_signature_hash(
                            Self::SIGNATURE,
                            topics.0,
                            Self::SIGNATURE_HASH,
                        ),
                    );
                }
                Ok(())
            }
            #[inline]
            fn tokenize_body(&self) -> Self::DataToken<'_> {
                (
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self.size),
                )
            }
            #[inline]
            fn topics(&self) -> <Self::TopicList as alloy_sol_types::SolType>::RustType {
                (Self::SIGNATURE_HASH.into(),)
            }
            #[inline]
            fn encode_topics_raw(
                &self,
                out: &mut [alloy_sol_types::abi::token::WordToken],
            ) -> alloy_sol_types::Result<()> {
                if out.len() < <Self::TopicList as alloy_sol_types::TopicList>::COUNT {
                    return Err(alloy_sol_types::Error::Overrun);
                }
                out[0usize] = alloy_sol_types::abi::token::WordToken(
                    Self::SIGNATURE_HASH,
                );
                Ok(())
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::private::IntoLogData for ArrayInitialized {
            fn to_log_data(&self) -> alloy_sol_types::private::LogData {
                From::from(self)
            }
            fn into_log_data(self) -> alloy_sol_types::private::LogData {
                From::from(&self)
            }
        }
        #[automatically_derived]
        impl From<&ArrayInitialized> for alloy_sol_types::private::LogData {
            #[inline]
            fn from(this: &ArrayInitialized) -> alloy_sol_types::private::LogData {
                alloy_sol_types::SolEvent::encode_log_data(this)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Event with signature `SumCalculated(uint256,uint256)` and selector `0xfd3dfbb3da06b2710848916c65866a3d0e050047402579a6e1714261137c19c6`.
```solidity
event SumCalculated(uint256 newSum, uint256 timestamp);
```*/
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    #[derive(Clone)]
    pub struct SumCalculated {
        #[allow(missing_docs)]
        pub newSum: alloy::sol_types::private::primitives::aliases::U256,
        #[allow(missing_docs)]
        pub timestamp: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        #[automatically_derived]
        impl alloy_sol_types::SolEvent for SumCalculated {
            type DataTuple<'a> = (
                alloy::sol_types::sol_data::Uint<256>,
                alloy::sol_types::sol_data::Uint<256>,
            );
            type DataToken<'a> = <Self::DataTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type TopicList = (alloy_sol_types::sol_data::FixedBytes<32>,);
            const SIGNATURE: &'static str = "SumCalculated(uint256,uint256)";
            const SIGNATURE_HASH: alloy_sol_types::private::B256 = alloy_sol_types::private::B256::new([
                253u8, 61u8, 251u8, 179u8, 218u8, 6u8, 178u8, 113u8, 8u8, 72u8, 145u8,
                108u8, 101u8, 134u8, 106u8, 61u8, 14u8, 5u8, 0u8, 71u8, 64u8, 37u8,
                121u8, 166u8, 225u8, 113u8, 66u8, 97u8, 19u8, 124u8, 25u8, 198u8,
            ]);
            const ANONYMOUS: bool = false;
            #[allow(unused_variables)]
            #[inline]
            fn new(
                topics: <Self::TopicList as alloy_sol_types::SolType>::RustType,
                data: <Self::DataTuple<'_> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                Self {
                    newSum: data.0,
                    timestamp: data.1,
                }
            }
            #[inline]
            fn check_signature(
                topics: &<Self::TopicList as alloy_sol_types::SolType>::RustType,
            ) -> alloy_sol_types::Result<()> {
                if topics.0 != Self::SIGNATURE_HASH {
                    return Err(
                        alloy_sol_types::Error::invalid_event_signature_hash(
                            Self::SIGNATURE,
                            topics.0,
                            Self::SIGNATURE_HASH,
                        ),
                    );
                }
                Ok(())
            }
            #[inline]
            fn tokenize_body(&self) -> Self::DataToken<'_> {
                (
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self.newSum),
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self.timestamp),
                )
            }
            #[inline]
            fn topics(&self) -> <Self::TopicList as alloy_sol_types::SolType>::RustType {
                (Self::SIGNATURE_HASH.into(),)
            }
            #[inline]
            fn encode_topics_raw(
                &self,
                out: &mut [alloy_sol_types::abi::token::WordToken],
            ) -> alloy_sol_types::Result<()> {
                if out.len() < <Self::TopicList as alloy_sol_types::TopicList>::COUNT {
                    return Err(alloy_sol_types::Error::Overrun);
                }
                out[0usize] = alloy_sol_types::abi::token::WordToken(
                    Self::SIGNATURE_HASH,
                );
                Ok(())
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::private::IntoLogData for SumCalculated {
            fn to_log_data(&self) -> alloy_sol_types::private::LogData {
                From::from(self)
            }
            fn into_log_data(self) -> alloy_sol_types::private::LogData {
                From::from(&self)
            }
        }
        #[automatically_derived]
        impl From<&SumCalculated> for alloy_sol_types::private::LogData {
            #[inline]
            fn from(this: &SumCalculated) -> alloy_sol_types::private::LogData {
                alloy_sol_types::SolEvent::encode_log_data(this)
            }
        }
    };
    /**Constructor`.
```solidity
constructor(address _avsAddress, address _blsSigChecker, uint256 _arraySize, uint256 _maxValue, uint256 _seed);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct constructorCall {
        #[allow(missing_docs)]
        pub _avsAddress: alloy::sol_types::private::Address,
        #[allow(missing_docs)]
        pub _blsSigChecker: alloy::sol_types::private::Address,
        #[allow(missing_docs)]
        pub _arraySize: alloy::sol_types::private::primitives::aliases::U256,
        #[allow(missing_docs)]
        pub _maxValue: alloy::sol_types::private::primitives::aliases::U256,
        #[allow(missing_docs)]
        pub _seed: alloy::sol_types::private::primitives::aliases::U256,
    }
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (
                alloy::sol_types::sol_data::Address,
                alloy::sol_types::sol_data::Address,
                alloy::sol_types::sol_data::Uint<256>,
                alloy::sol_types::sol_data::Uint<256>,
                alloy::sol_types::sol_data::Uint<256>,
            );
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::Address,
                alloy::sol_types::private::Address,
                alloy::sol_types::private::primitives::aliases::U256,
                alloy::sol_types::private::primitives::aliases::U256,
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<constructorCall> for UnderlyingRustTuple<'_> {
                fn from(value: constructorCall) -> Self {
                    (
                        value._avsAddress,
                        value._blsSigChecker,
                        value._arraySize,
                        value._maxValue,
                        value._seed,
                    )
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for constructorCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {
                        _avsAddress: tuple.0,
                        _blsSigChecker: tuple.1,
                        _arraySize: tuple.2,
                        _maxValue: tuple.3,
                        _seed: tuple.4,
                    }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolConstructor for constructorCall {
            type Parameters<'a> = (
                alloy::sol_types::sol_data::Address,
                alloy::sol_types::sol_data::Address,
                alloy::sol_types::sol_data::Uint<256>,
                alloy::sol_types::sol_data::Uint<256>,
                alloy::sol_types::sol_data::Uint<256>,
            );
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                (
                    <alloy::sol_types::sol_data::Address as alloy_sol_types::SolType>::tokenize(
                        &self._avsAddress,
                    ),
                    <alloy::sol_types::sol_data::Address as alloy_sol_types::SolType>::tokenize(
                        &self._blsSigChecker,
                    ),
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self._arraySize),
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self._maxValue),
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self._seed),
                )
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `BLOCK_STALE_MEASURE()` and selector `0x5e8b3f2d`.
```solidity
function BLOCK_STALE_MEASURE() external view returns (uint32);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct BLOCK_STALE_MEASURECall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`BLOCK_STALE_MEASURE()`](BLOCK_STALE_MEASURECall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct BLOCK_STALE_MEASUREReturn {
        #[allow(missing_docs)]
        pub _0: u32,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<BLOCK_STALE_MEASURECall>
            for UnderlyingRustTuple<'_> {
                fn from(value: BLOCK_STALE_MEASURECall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for BLOCK_STALE_MEASURECall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<32>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (u32,);
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<BLOCK_STALE_MEASUREReturn>
            for UnderlyingRustTuple<'_> {
                fn from(value: BLOCK_STALE_MEASUREReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for BLOCK_STALE_MEASUREReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for BLOCK_STALE_MEASURECall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = BLOCK_STALE_MEASUREReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Uint<32>,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "BLOCK_STALE_MEASURE()";
            const SELECTOR: [u8; 4] = [94u8, 139u8, 63u8, 45u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `QUORUM_THRESHOLD()` and selector `0x5e510b60`.
```solidity
function QUORUM_THRESHOLD() external view returns (uint8);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct QUORUM_THRESHOLDCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`QUORUM_THRESHOLD()`](QUORUM_THRESHOLDCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct QUORUM_THRESHOLDReturn {
        #[allow(missing_docs)]
        pub _0: u8,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<QUORUM_THRESHOLDCall>
            for UnderlyingRustTuple<'_> {
                fn from(value: QUORUM_THRESHOLDCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for QUORUM_THRESHOLDCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<8>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (u8,);
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<QUORUM_THRESHOLDReturn>
            for UnderlyingRustTuple<'_> {
                fn from(value: QUORUM_THRESHOLDReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for QUORUM_THRESHOLDReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for QUORUM_THRESHOLDCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = QUORUM_THRESHOLDReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Uint<8>,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "QUORUM_THRESHOLD()";
            const SELECTOR: [u8; 4] = [94u8, 81u8, 11u8, 96u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `THRESHOLD_DENOMINATOR()` and selector `0xef024458`.
```solidity
function THRESHOLD_DENOMINATOR() external view returns (uint8);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct THRESHOLD_DENOMINATORCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`THRESHOLD_DENOMINATOR()`](THRESHOLD_DENOMINATORCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct THRESHOLD_DENOMINATORReturn {
        #[allow(missing_docs)]
        pub _0: u8,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<THRESHOLD_DENOMINATORCall>
            for UnderlyingRustTuple<'_> {
                fn from(value: THRESHOLD_DENOMINATORCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for THRESHOLD_DENOMINATORCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<8>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (u8,);
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<THRESHOLD_DENOMINATORReturn>
            for UnderlyingRustTuple<'_> {
                fn from(value: THRESHOLD_DENOMINATORReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for THRESHOLD_DENOMINATORReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for THRESHOLD_DENOMINATORCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = THRESHOLD_DENOMINATORReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Uint<8>,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "THRESHOLD_DENOMINATOR()";
            const SELECTOR: [u8; 4] = [239u8, 2u8, 68u8, 88u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `arraySize()` and selector `0xe0f6ff43`.
```solidity
function arraySize() external view returns (uint256);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct arraySizeCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`arraySize()`](arraySizeCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct arraySizeReturn {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<arraySizeCall> for UnderlyingRustTuple<'_> {
                fn from(value: arraySizeCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for arraySizeCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<arraySizeReturn> for UnderlyingRustTuple<'_> {
                fn from(value: arraySizeReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for arraySizeReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for arraySizeCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = arraySizeReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "arraySize()";
            const SELECTOR: [u8; 4] = [224u8, 246u8, 255u8, 67u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `avsAddress()` and selector `0xda324c13`.
```solidity
function avsAddress() external view returns (address);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct avsAddressCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`avsAddress()`](avsAddressCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct avsAddressReturn {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::Address,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<avsAddressCall> for UnderlyingRustTuple<'_> {
                fn from(value: avsAddressCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for avsAddressCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Address,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (alloy::sol_types::private::Address,);
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<avsAddressReturn> for UnderlyingRustTuple<'_> {
                fn from(value: avsAddressReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for avsAddressReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for avsAddressCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = avsAddressReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Address,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "avsAddress()";
            const SELECTOR: [u8; 4] = [218u8, 50u8, 76u8, 19u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `blsSignatureChecker()` and selector `0x1c178e9c`.
```solidity
function blsSignatureChecker() external view returns (address);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct blsSignatureCheckerCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`blsSignatureChecker()`](blsSignatureCheckerCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct blsSignatureCheckerReturn {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::Address,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<blsSignatureCheckerCall>
            for UnderlyingRustTuple<'_> {
                fn from(value: blsSignatureCheckerCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for blsSignatureCheckerCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Address,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (alloy::sol_types::private::Address,);
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<blsSignatureCheckerReturn>
            for UnderlyingRustTuple<'_> {
                fn from(value: blsSignatureCheckerReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for blsSignatureCheckerReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for blsSignatureCheckerCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = blsSignatureCheckerReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Address,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "blsSignatureChecker()";
            const SELECTOR: [u8; 4] = [28u8, 23u8, 142u8, 156u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `currentSum()` and selector `0x8b97c23d`.
```solidity
function currentSum() external view returns (uint256);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct currentSumCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`currentSum()`](currentSumCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct currentSumReturn {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<currentSumCall> for UnderlyingRustTuple<'_> {
                fn from(value: currentSumCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for currentSumCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<currentSumReturn> for UnderlyingRustTuple<'_> {
                fn from(value: currentSumReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for currentSumReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for currentSumCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = currentSumReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "currentSum()";
            const SELECTOR: [u8; 4] = [139u8, 151u8, 194u8, 61u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `getArrayElement(uint256)` and selector `0x142edc7a`.
```solidity
function getArrayElement(uint256 index) external view returns (uint256);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct getArrayElementCall {
        #[allow(missing_docs)]
        pub index: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`getArrayElement(uint256)`](getArrayElementCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct getArrayElementReturn {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<getArrayElementCall> for UnderlyingRustTuple<'_> {
                fn from(value: getArrayElementCall) -> Self {
                    (value.index,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for getArrayElementCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { index: tuple.0 }
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<getArrayElementReturn>
            for UnderlyingRustTuple<'_> {
                fn from(value: getArrayElementReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for getArrayElementReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for getArrayElementCall {
            type Parameters<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = getArrayElementReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "getArrayElement(uint256)";
            const SELECTOR: [u8; 4] = [20u8, 46u8, 220u8, 122u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                (
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self.index),
                )
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `getArrayLength()` and selector `0x0849cc99`.
```solidity
function getArrayLength() external view returns (uint256);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct getArrayLengthCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`getArrayLength()`](getArrayLengthCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct getArrayLengthReturn {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<getArrayLengthCall> for UnderlyingRustTuple<'_> {
                fn from(value: getArrayLengthCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for getArrayLengthCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<getArrayLengthReturn>
            for UnderlyingRustTuple<'_> {
                fn from(value: getArrayLengthReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for getArrayLengthReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for getArrayLengthCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = getArrayLengthReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "getArrayLength()";
            const SELECTOR: [u8; 4] = [8u8, 73u8, 204u8, 153u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `getFullArray()` and selector `0x331f2300`.
```solidity
function getFullArray() external view returns (uint256[] memory);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct getFullArrayCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`getFullArray()`](getFullArrayCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct getFullArrayReturn {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::Vec<
            alloy::sol_types::private::primitives::aliases::U256,
        >,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<getFullArrayCall> for UnderlyingRustTuple<'_> {
                fn from(value: getFullArrayCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for getFullArrayCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (
                alloy::sol_types::sol_data::Array<alloy::sol_types::sol_data::Uint<256>>,
            );
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::Vec<
                    alloy::sol_types::private::primitives::aliases::U256,
                >,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<getFullArrayReturn> for UnderlyingRustTuple<'_> {
                fn from(value: getFullArrayReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for getFullArrayReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for getFullArrayCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = getFullArrayReturn;
            type ReturnTuple<'a> = (
                alloy::sol_types::sol_data::Array<alloy::sol_types::sol_data::Uint<256>>,
            );
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "getFullArray()";
            const SELECTOR: [u8; 4] = [51u8, 31u8, 35u8, 0u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `maxValue()` and selector `0x94a5c2e4`.
```solidity
function maxValue() external view returns (uint256);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct maxValueCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`maxValue()`](maxValueCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct maxValueReturn {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<maxValueCall> for UnderlyingRustTuple<'_> {
                fn from(value: maxValueCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for maxValueCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<maxValueReturn> for UnderlyingRustTuple<'_> {
                fn from(value: maxValueReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for maxValueReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for maxValueCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = maxValueReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "maxValue()";
            const SELECTOR: [u8; 4] = [148u8, 165u8, 194u8, 228u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `namespace()` and selector `0x7c015a89`.
```solidity
function namespace() external view returns (bytes memory);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct namespaceCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`namespace()`](namespaceCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct namespaceReturn {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::Bytes,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<namespaceCall> for UnderlyingRustTuple<'_> {
                fn from(value: namespaceCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for namespaceCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Bytes,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (alloy::sol_types::private::Bytes,);
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<namespaceReturn> for UnderlyingRustTuple<'_> {
                fn from(value: namespaceReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for namespaceReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for namespaceCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = namespaceReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Bytes,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "namespace()";
            const SELECTOR: [u8; 4] = [124u8, 1u8, 90u8, 137u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `resetArray(uint256)` and selector `0x5aff4e2e`.
```solidity
function resetArray(uint256 _seed) external;
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct resetArrayCall {
        #[allow(missing_docs)]
        pub _seed: alloy::sol_types::private::primitives::aliases::U256,
    }
    ///Container type for the return parameters of the [`resetArray(uint256)`](resetArrayCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct resetArrayReturn {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<resetArrayCall> for UnderlyingRustTuple<'_> {
                fn from(value: resetArrayCall) -> Self {
                    (value._seed,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for resetArrayCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _seed: tuple.0 }
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<resetArrayReturn> for UnderlyingRustTuple<'_> {
                fn from(value: resetArrayReturn) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for resetArrayReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for resetArrayCall {
            type Parameters<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = resetArrayReturn;
            type ReturnTuple<'a> = ();
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "resetArray(uint256)";
            const SELECTOR: [u8; 4] = [90u8, 255u8, 78u8, 46u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                (
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self._seed),
                )
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `setArrayElement(uint256,uint256)` and selector `0xa27b23a1`.
```solidity
function setArrayElement(uint256 index, uint256 newValue) external;
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct setArrayElementCall {
        #[allow(missing_docs)]
        pub index: alloy::sol_types::private::primitives::aliases::U256,
        #[allow(missing_docs)]
        pub newValue: alloy::sol_types::private::primitives::aliases::U256,
    }
    ///Container type for the return parameters of the [`setArrayElement(uint256,uint256)`](setArrayElementCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct setArrayElementReturn {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (
                alloy::sol_types::sol_data::Uint<256>,
                alloy::sol_types::sol_data::Uint<256>,
            );
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<setArrayElementCall> for UnderlyingRustTuple<'_> {
                fn from(value: setArrayElementCall) -> Self {
                    (value.index, value.newValue)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for setArrayElementCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {
                        index: tuple.0,
                        newValue: tuple.1,
                    }
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<setArrayElementReturn>
            for UnderlyingRustTuple<'_> {
                fn from(value: setArrayElementReturn) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for setArrayElementReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for setArrayElementCall {
            type Parameters<'a> = (
                alloy::sol_types::sol_data::Uint<256>,
                alloy::sol_types::sol_data::Uint<256>,
            );
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = setArrayElementReturn;
            type ReturnTuple<'a> = ();
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "setArrayElement(uint256,uint256)";
            const SELECTOR: [u8; 4] = [162u8, 123u8, 35u8, 161u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                (
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self.index),
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self.newValue),
                )
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `stateTransitionCount()` and selector `0xf4833e20`.
```solidity
function stateTransitionCount() external view returns (uint256 count);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct stateTransitionCountCall {}
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`stateTransitionCount()`](stateTransitionCountCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct stateTransitionCountReturn {
        #[allow(missing_docs)]
        pub count: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<stateTransitionCountCall>
            for UnderlyingRustTuple<'_> {
                fn from(value: stateTransitionCountCall) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for stateTransitionCountCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<stateTransitionCountReturn>
            for UnderlyingRustTuple<'_> {
                fn from(value: stateTransitionCountReturn) -> Self {
                    (value.count,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for stateTransitionCountReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { count: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for stateTransitionCountCall {
            type Parameters<'a> = ();
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = stateTransitionCountReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "stateTransitionCount()";
            const SELECTOR: [u8; 4] = [244u8, 131u8, 62u8, 32u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                ()
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `sum(uint256[])` and selector `0x0194db8e`.
```solidity
function sum(uint256[] memory indexes) external;
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct sumCall {
        #[allow(missing_docs)]
        pub indexes: alloy::sol_types::private::Vec<
            alloy::sol_types::private::primitives::aliases::U256,
        >,
    }
    ///Container type for the return parameters of the [`sum(uint256[])`](sumCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct sumReturn {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (
                alloy::sol_types::sol_data::Array<alloy::sol_types::sol_data::Uint<256>>,
            );
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::Vec<
                    alloy::sol_types::private::primitives::aliases::U256,
                >,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<sumCall> for UnderlyingRustTuple<'_> {
                fn from(value: sumCall) -> Self {
                    (value.indexes,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for sumCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { indexes: tuple.0 }
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<sumReturn> for UnderlyingRustTuple<'_> {
                fn from(value: sumReturn) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for sumReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for sumCall {
            type Parameters<'a> = (
                alloy::sol_types::sol_data::Array<alloy::sol_types::sol_data::Uint<256>>,
            );
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = sumReturn;
            type ReturnTuple<'a> = ();
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "sum(uint256[])";
            const SELECTOR: [u8; 4] = [1u8, 148u8, 219u8, 142u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                (
                    <alloy::sol_types::sol_data::Array<
                        alloy::sol_types::sol_data::Uint<256>,
                    > as alloy_sol_types::SolType>::tokenize(&self.indexes),
                )
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `values(uint256)` and selector `0x5e383d21`.
```solidity
function values(uint256) external view returns (uint256);
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct valuesCall {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    ///Container type for the return parameters of the [`values(uint256)`](valuesCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct valuesReturn {
        #[allow(missing_docs)]
        pub _0: alloy::sol_types::private::primitives::aliases::U256,
    }
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<valuesCall> for UnderlyingRustTuple<'_> {
                fn from(value: valuesCall) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for valuesCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::primitives::aliases::U256,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<valuesReturn> for UnderlyingRustTuple<'_> {
                fn from(value: valuesReturn) -> Self {
                    (value._0,)
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for valuesReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self { _0: tuple.0 }
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for valuesCall {
            type Parameters<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = valuesReturn;
            type ReturnTuple<'a> = (alloy::sol_types::sol_data::Uint<256>,);
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "values(uint256)";
            const SELECTOR: [u8; 4] = [94u8, 56u8, 61u8, 33u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                (
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self._0),
                )
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Default, Debug, PartialEq, Eq, Hash)]
    /**Function with signature `verifyAndUpdate(bytes32,bytes,uint32,bytes,uint256,address,bytes4,(uint32[],(uint256,uint256)[],(uint256,uint256)[],(uint256[2],uint256[2]),(uint256,uint256),uint32[],uint32[],uint32[][]))` and selector `0x8f0bb7cc`.
```solidity
function verifyAndUpdate(bytes32 msgHash, bytes memory quorumNumbers, uint32 referenceBlockNumber, bytes memory storageUpdates, uint256 transitionIndex, address targetAddr, bytes4 targetFunction, IBLSSignatureCheckerTypes.NonSignerStakesAndSignature memory nonSignerStakesAndSignature) external;
```*/
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct verifyAndUpdateCall {
        #[allow(missing_docs)]
        pub msgHash: alloy::sol_types::private::FixedBytes<32>,
        #[allow(missing_docs)]
        pub quorumNumbers: alloy::sol_types::private::Bytes,
        #[allow(missing_docs)]
        pub referenceBlockNumber: u32,
        #[allow(missing_docs)]
        pub storageUpdates: alloy::sol_types::private::Bytes,
        #[allow(missing_docs)]
        pub transitionIndex: alloy::sol_types::private::primitives::aliases::U256,
        #[allow(missing_docs)]
        pub targetAddr: alloy::sol_types::private::Address,
        #[allow(missing_docs)]
        pub targetFunction: alloy::sol_types::private::FixedBytes<4>,
        #[allow(missing_docs)]
        pub nonSignerStakesAndSignature: <IBLSSignatureCheckerTypes::NonSignerStakesAndSignature as alloy::sol_types::SolType>::RustType,
    }
    ///Container type for the return parameters of the [`verifyAndUpdate(bytes32,bytes,uint32,bytes,uint256,address,bytes4,(uint32[],(uint256,uint256)[],(uint256,uint256)[],(uint256[2],uint256[2]),(uint256,uint256),uint32[],uint32[],uint32[][]))`](verifyAndUpdateCall) function.
    #[allow(non_camel_case_types, non_snake_case, clippy::pub_underscore_fields)]
    #[derive(Clone)]
    pub struct verifyAndUpdateReturn {}
    #[allow(
        non_camel_case_types,
        non_snake_case,
        clippy::pub_underscore_fields,
        clippy::style
    )]
    const _: () = {
        use alloy::sol_types as alloy_sol_types;
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = (
                alloy::sol_types::sol_data::FixedBytes<32>,
                alloy::sol_types::sol_data::Bytes,
                alloy::sol_types::sol_data::Uint<32>,
                alloy::sol_types::sol_data::Bytes,
                alloy::sol_types::sol_data::Uint<256>,
                alloy::sol_types::sol_data::Address,
                alloy::sol_types::sol_data::FixedBytes<4>,
                IBLSSignatureCheckerTypes::NonSignerStakesAndSignature,
            );
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = (
                alloy::sol_types::private::FixedBytes<32>,
                alloy::sol_types::private::Bytes,
                u32,
                alloy::sol_types::private::Bytes,
                alloy::sol_types::private::primitives::aliases::U256,
                alloy::sol_types::private::Address,
                alloy::sol_types::private::FixedBytes<4>,
                <IBLSSignatureCheckerTypes::NonSignerStakesAndSignature as alloy::sol_types::SolType>::RustType,
            );
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<verifyAndUpdateCall> for UnderlyingRustTuple<'_> {
                fn from(value: verifyAndUpdateCall) -> Self {
                    (
                        value.msgHash,
                        value.quorumNumbers,
                        value.referenceBlockNumber,
                        value.storageUpdates,
                        value.transitionIndex,
                        value.targetAddr,
                        value.targetFunction,
                        value.nonSignerStakesAndSignature,
                    )
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>> for verifyAndUpdateCall {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {
                        msgHash: tuple.0,
                        quorumNumbers: tuple.1,
                        referenceBlockNumber: tuple.2,
                        storageUpdates: tuple.3,
                        transitionIndex: tuple.4,
                        targetAddr: tuple.5,
                        targetFunction: tuple.6,
                        nonSignerStakesAndSignature: tuple.7,
                    }
                }
            }
        }
        {
            #[doc(hidden)]
            type UnderlyingSolTuple<'a> = ();
            #[doc(hidden)]
            type UnderlyingRustTuple<'a> = ();
            #[cfg(test)]
            #[allow(dead_code, unreachable_patterns)]
            fn _type_assertion(
                _t: alloy_sol_types::private::AssertTypeEq<UnderlyingRustTuple>,
            ) {
                match _t {
                    alloy_sol_types::private::AssertTypeEq::<
                        <UnderlyingSolTuple as alloy_sol_types::SolType>::RustType,
                    >(_) => {}
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<verifyAndUpdateReturn>
            for UnderlyingRustTuple<'_> {
                fn from(value: verifyAndUpdateReturn) -> Self {
                    ()
                }
            }
            #[automatically_derived]
            #[doc(hidden)]
            impl ::core::convert::From<UnderlyingRustTuple<'_>>
            for verifyAndUpdateReturn {
                fn from(tuple: UnderlyingRustTuple<'_>) -> Self {
                    Self {}
                }
            }
        }
        #[automatically_derived]
        impl alloy_sol_types::SolCall for verifyAndUpdateCall {
            type Parameters<'a> = (
                alloy::sol_types::sol_data::FixedBytes<32>,
                alloy::sol_types::sol_data::Bytes,
                alloy::sol_types::sol_data::Uint<32>,
                alloy::sol_types::sol_data::Bytes,
                alloy::sol_types::sol_data::Uint<256>,
                alloy::sol_types::sol_data::Address,
                alloy::sol_types::sol_data::FixedBytes<4>,
                IBLSSignatureCheckerTypes::NonSignerStakesAndSignature,
            );
            type Token<'a> = <Self::Parameters<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            type Return = verifyAndUpdateReturn;
            type ReturnTuple<'a> = ();
            type ReturnToken<'a> = <Self::ReturnTuple<
                'a,
            > as alloy_sol_types::SolType>::Token<'a>;
            const SIGNATURE: &'static str = "verifyAndUpdate(bytes32,bytes,uint32,bytes,uint256,address,bytes4,(uint32[],(uint256,uint256)[],(uint256,uint256)[],(uint256[2],uint256[2]),(uint256,uint256),uint32[],uint32[],uint32[][]))";
            const SELECTOR: [u8; 4] = [143u8, 11u8, 183u8, 204u8];
            #[inline]
            fn new<'a>(
                tuple: <Self::Parameters<'a> as alloy_sol_types::SolType>::RustType,
            ) -> Self {
                tuple.into()
            }
            #[inline]
            fn tokenize(&self) -> Self::Token<'_> {
                (
                    <alloy::sol_types::sol_data::FixedBytes<
                        32,
                    > as alloy_sol_types::SolType>::tokenize(&self.msgHash),
                    <alloy::sol_types::sol_data::Bytes as alloy_sol_types::SolType>::tokenize(
                        &self.quorumNumbers,
                    ),
                    <alloy::sol_types::sol_data::Uint<
                        32,
                    > as alloy_sol_types::SolType>::tokenize(&self.referenceBlockNumber),
                    <alloy::sol_types::sol_data::Bytes as alloy_sol_types::SolType>::tokenize(
                        &self.storageUpdates,
                    ),
                    <alloy::sol_types::sol_data::Uint<
                        256,
                    > as alloy_sol_types::SolType>::tokenize(&self.transitionIndex),
                    <alloy::sol_types::sol_data::Address as alloy_sol_types::SolType>::tokenize(
                        &self.targetAddr,
                    ),
                    <alloy::sol_types::sol_data::FixedBytes<
                        4,
                    > as alloy_sol_types::SolType>::tokenize(&self.targetFunction),
                    <IBLSSignatureCheckerTypes::NonSignerStakesAndSignature as alloy_sol_types::SolType>::tokenize(
                        &self.nonSignerStakesAndSignature,
                    ),
                )
            }
            #[inline]
            fn abi_decode_returns(
                data: &[u8],
                validate: bool,
            ) -> alloy_sol_types::Result<Self::Return> {
                <Self::ReturnTuple<
                    '_,
                > as alloy_sol_types::SolType>::abi_decode_sequence(data, validate)
                    .map(Into::into)
            }
        }
    };
    ///Container for all the [`ArraySummation`](self) function calls.
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive()]
    pub enum ArraySummationCalls {
        #[allow(missing_docs)]
        BLOCK_STALE_MEASURE(BLOCK_STALE_MEASURECall),
        #[allow(missing_docs)]
        QUORUM_THRESHOLD(QUORUM_THRESHOLDCall),
        #[allow(missing_docs)]
        THRESHOLD_DENOMINATOR(THRESHOLD_DENOMINATORCall),
        #[allow(missing_docs)]
        arraySize(arraySizeCall),
        #[allow(missing_docs)]
        avsAddress(avsAddressCall),
        #[allow(missing_docs)]
        blsSignatureChecker(blsSignatureCheckerCall),
        #[allow(missing_docs)]
        currentSum(currentSumCall),
        #[allow(missing_docs)]
        getArrayElement(getArrayElementCall),
        #[allow(missing_docs)]
        getArrayLength(getArrayLengthCall),
        #[allow(missing_docs)]
        getFullArray(getFullArrayCall),
        #[allow(missing_docs)]
        maxValue(maxValueCall),
        #[allow(missing_docs)]
        namespace(namespaceCall),
        #[allow(missing_docs)]
        resetArray(resetArrayCall),
        #[allow(missing_docs)]
        setArrayElement(setArrayElementCall),
        #[allow(missing_docs)]
        stateTransitionCount(stateTransitionCountCall),
        #[allow(missing_docs)]
        sum(sumCall),
        #[allow(missing_docs)]
        values(valuesCall),
        #[allow(missing_docs)]
        verifyAndUpdate(verifyAndUpdateCall),
    }
    #[automatically_derived]
    impl ArraySummationCalls {
        /// All the selectors of this enum.
        ///
        /// Note that the selectors might not be in the same order as the variants.
        /// No guarantees are made about the order of the selectors.
        ///
        /// Prefer using `SolInterface` methods instead.
        pub const SELECTORS: &'static [[u8; 4usize]] = &[
            [1u8, 148u8, 219u8, 142u8],
            [8u8, 73u8, 204u8, 153u8],
            [20u8, 46u8, 220u8, 122u8],
            [28u8, 23u8, 142u8, 156u8],
            [51u8, 31u8, 35u8, 0u8],
            [90u8, 255u8, 78u8, 46u8],
            [94u8, 56u8, 61u8, 33u8],
            [94u8, 81u8, 11u8, 96u8],
            [94u8, 139u8, 63u8, 45u8],
            [124u8, 1u8, 90u8, 137u8],
            [139u8, 151u8, 194u8, 61u8],
            [143u8, 11u8, 183u8, 204u8],
            [148u8, 165u8, 194u8, 228u8],
            [162u8, 123u8, 35u8, 161u8],
            [218u8, 50u8, 76u8, 19u8],
            [224u8, 246u8, 255u8, 67u8],
            [239u8, 2u8, 68u8, 88u8],
            [244u8, 131u8, 62u8, 32u8],
        ];
    }
    #[automatically_derived]
    impl alloy_sol_types::SolInterface for ArraySummationCalls {
        const NAME: &'static str = "ArraySummationCalls";
        const MIN_DATA_LENGTH: usize = 0usize;
        const COUNT: usize = 18usize;
        #[inline]
        fn selector(&self) -> [u8; 4] {
            match self {
                Self::BLOCK_STALE_MEASURE(_) => {
                    <BLOCK_STALE_MEASURECall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::QUORUM_THRESHOLD(_) => {
                    <QUORUM_THRESHOLDCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::THRESHOLD_DENOMINATOR(_) => {
                    <THRESHOLD_DENOMINATORCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::arraySize(_) => {
                    <arraySizeCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::avsAddress(_) => {
                    <avsAddressCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::blsSignatureChecker(_) => {
                    <blsSignatureCheckerCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::currentSum(_) => {
                    <currentSumCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::getArrayElement(_) => {
                    <getArrayElementCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::getArrayLength(_) => {
                    <getArrayLengthCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::getFullArray(_) => {
                    <getFullArrayCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::maxValue(_) => <maxValueCall as alloy_sol_types::SolCall>::SELECTOR,
                Self::namespace(_) => {
                    <namespaceCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::resetArray(_) => {
                    <resetArrayCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::setArrayElement(_) => {
                    <setArrayElementCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::stateTransitionCount(_) => {
                    <stateTransitionCountCall as alloy_sol_types::SolCall>::SELECTOR
                }
                Self::sum(_) => <sumCall as alloy_sol_types::SolCall>::SELECTOR,
                Self::values(_) => <valuesCall as alloy_sol_types::SolCall>::SELECTOR,
                Self::verifyAndUpdate(_) => {
                    <verifyAndUpdateCall as alloy_sol_types::SolCall>::SELECTOR
                }
            }
        }
        #[inline]
        fn selector_at(i: usize) -> ::core::option::Option<[u8; 4]> {
            Self::SELECTORS.get(i).copied()
        }
        #[inline]
        fn valid_selector(selector: [u8; 4]) -> bool {
            Self::SELECTORS.binary_search(&selector).is_ok()
        }
        #[inline]
        #[allow(non_snake_case)]
        fn abi_decode_raw(
            selector: [u8; 4],
            data: &[u8],
            validate: bool,
        ) -> alloy_sol_types::Result<Self> {
            static DECODE_SHIMS: &[fn(
                &[u8],
                bool,
            ) -> alloy_sol_types::Result<ArraySummationCalls>] = &[
                {
                    fn sum(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <sumCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::sum)
                    }
                    sum
                },
                {
                    fn getArrayLength(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <getArrayLengthCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::getArrayLength)
                    }
                    getArrayLength
                },
                {
                    fn getArrayElement(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <getArrayElementCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::getArrayElement)
                    }
                    getArrayElement
                },
                {
                    fn blsSignatureChecker(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <blsSignatureCheckerCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::blsSignatureChecker)
                    }
                    blsSignatureChecker
                },
                {
                    fn getFullArray(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <getFullArrayCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::getFullArray)
                    }
                    getFullArray
                },
                {
                    fn resetArray(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <resetArrayCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::resetArray)
                    }
                    resetArray
                },
                {
                    fn values(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <valuesCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::values)
                    }
                    values
                },
                {
                    fn QUORUM_THRESHOLD(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <QUORUM_THRESHOLDCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::QUORUM_THRESHOLD)
                    }
                    QUORUM_THRESHOLD
                },
                {
                    fn BLOCK_STALE_MEASURE(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <BLOCK_STALE_MEASURECall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::BLOCK_STALE_MEASURE)
                    }
                    BLOCK_STALE_MEASURE
                },
                {
                    fn namespace(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <namespaceCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::namespace)
                    }
                    namespace
                },
                {
                    fn currentSum(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <currentSumCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::currentSum)
                    }
                    currentSum
                },
                {
                    fn verifyAndUpdate(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <verifyAndUpdateCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::verifyAndUpdate)
                    }
                    verifyAndUpdate
                },
                {
                    fn maxValue(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <maxValueCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::maxValue)
                    }
                    maxValue
                },
                {
                    fn setArrayElement(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <setArrayElementCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::setArrayElement)
                    }
                    setArrayElement
                },
                {
                    fn avsAddress(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <avsAddressCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::avsAddress)
                    }
                    avsAddress
                },
                {
                    fn arraySize(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <arraySizeCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::arraySize)
                    }
                    arraySize
                },
                {
                    fn THRESHOLD_DENOMINATOR(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <THRESHOLD_DENOMINATORCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::THRESHOLD_DENOMINATOR)
                    }
                    THRESHOLD_DENOMINATOR
                },
                {
                    fn stateTransitionCount(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationCalls> {
                        <stateTransitionCountCall as alloy_sol_types::SolCall>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationCalls::stateTransitionCount)
                    }
                    stateTransitionCount
                },
            ];
            let Ok(idx) = Self::SELECTORS.binary_search(&selector) else {
                return Err(
                    alloy_sol_types::Error::unknown_selector(
                        <Self as alloy_sol_types::SolInterface>::NAME,
                        selector,
                    ),
                );
            };
            DECODE_SHIMS[idx](data, validate)
        }
        #[inline]
        fn abi_encoded_size(&self) -> usize {
            match self {
                Self::BLOCK_STALE_MEASURE(inner) => {
                    <BLOCK_STALE_MEASURECall as alloy_sol_types::SolCall>::abi_encoded_size(
                        inner,
                    )
                }
                Self::QUORUM_THRESHOLD(inner) => {
                    <QUORUM_THRESHOLDCall as alloy_sol_types::SolCall>::abi_encoded_size(
                        inner,
                    )
                }
                Self::THRESHOLD_DENOMINATOR(inner) => {
                    <THRESHOLD_DENOMINATORCall as alloy_sol_types::SolCall>::abi_encoded_size(
                        inner,
                    )
                }
                Self::arraySize(inner) => {
                    <arraySizeCall as alloy_sol_types::SolCall>::abi_encoded_size(inner)
                }
                Self::avsAddress(inner) => {
                    <avsAddressCall as alloy_sol_types::SolCall>::abi_encoded_size(inner)
                }
                Self::blsSignatureChecker(inner) => {
                    <blsSignatureCheckerCall as alloy_sol_types::SolCall>::abi_encoded_size(
                        inner,
                    )
                }
                Self::currentSum(inner) => {
                    <currentSumCall as alloy_sol_types::SolCall>::abi_encoded_size(inner)
                }
                Self::getArrayElement(inner) => {
                    <getArrayElementCall as alloy_sol_types::SolCall>::abi_encoded_size(
                        inner,
                    )
                }
                Self::getArrayLength(inner) => {
                    <getArrayLengthCall as alloy_sol_types::SolCall>::abi_encoded_size(
                        inner,
                    )
                }
                Self::getFullArray(inner) => {
                    <getFullArrayCall as alloy_sol_types::SolCall>::abi_encoded_size(
                        inner,
                    )
                }
                Self::maxValue(inner) => {
                    <maxValueCall as alloy_sol_types::SolCall>::abi_encoded_size(inner)
                }
                Self::namespace(inner) => {
                    <namespaceCall as alloy_sol_types::SolCall>::abi_encoded_size(inner)
                }
                Self::resetArray(inner) => {
                    <resetArrayCall as alloy_sol_types::SolCall>::abi_encoded_size(inner)
                }
                Self::setArrayElement(inner) => {
                    <setArrayElementCall as alloy_sol_types::SolCall>::abi_encoded_size(
                        inner,
                    )
                }
                Self::stateTransitionCount(inner) => {
                    <stateTransitionCountCall as alloy_sol_types::SolCall>::abi_encoded_size(
                        inner,
                    )
                }
                Self::sum(inner) => {
                    <sumCall as alloy_sol_types::SolCall>::abi_encoded_size(inner)
                }
                Self::values(inner) => {
                    <valuesCall as alloy_sol_types::SolCall>::abi_encoded_size(inner)
                }
                Self::verifyAndUpdate(inner) => {
                    <verifyAndUpdateCall as alloy_sol_types::SolCall>::abi_encoded_size(
                        inner,
                    )
                }
            }
        }
        #[inline]
        fn abi_encode_raw(&self, out: &mut alloy_sol_types::private::Vec<u8>) {
            match self {
                Self::BLOCK_STALE_MEASURE(inner) => {
                    <BLOCK_STALE_MEASURECall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::QUORUM_THRESHOLD(inner) => {
                    <QUORUM_THRESHOLDCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::THRESHOLD_DENOMINATOR(inner) => {
                    <THRESHOLD_DENOMINATORCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::arraySize(inner) => {
                    <arraySizeCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::avsAddress(inner) => {
                    <avsAddressCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::blsSignatureChecker(inner) => {
                    <blsSignatureCheckerCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::currentSum(inner) => {
                    <currentSumCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::getArrayElement(inner) => {
                    <getArrayElementCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::getArrayLength(inner) => {
                    <getArrayLengthCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::getFullArray(inner) => {
                    <getFullArrayCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::maxValue(inner) => {
                    <maxValueCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::namespace(inner) => {
                    <namespaceCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::resetArray(inner) => {
                    <resetArrayCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::setArrayElement(inner) => {
                    <setArrayElementCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::stateTransitionCount(inner) => {
                    <stateTransitionCountCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::sum(inner) => {
                    <sumCall as alloy_sol_types::SolCall>::abi_encode_raw(inner, out)
                }
                Self::values(inner) => {
                    <valuesCall as alloy_sol_types::SolCall>::abi_encode_raw(inner, out)
                }
                Self::verifyAndUpdate(inner) => {
                    <verifyAndUpdateCall as alloy_sol_types::SolCall>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
            }
        }
    }
    ///Container for all the [`ArraySummation`](self) custom errors.
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub enum ArraySummationErrors {
        #[allow(missing_docs)]
        FutureBlockNumber(FutureBlockNumber),
        #[allow(missing_docs)]
        InsufficientQuorumThreshold(InsufficientQuorumThreshold),
        #[allow(missing_docs)]
        InvalidArguments(InvalidArguments),
        #[allow(missing_docs)]
        InvalidConfiguration(InvalidConfiguration),
        #[allow(missing_docs)]
        InvalidOperation(InvalidOperation),
        #[allow(missing_docs)]
        InvalidSignature(InvalidSignature),
        #[allow(missing_docs)]
        InvalidStorageUpdates(InvalidStorageUpdates),
        #[allow(missing_docs)]
        InvalidTransitionIndex(InvalidTransitionIndex),
        #[allow(missing_docs)]
        RevertingContext(RevertingContext),
        #[allow(missing_docs)]
        StaleBlockNumber(StaleBlockNumber),
    }
    #[automatically_derived]
    impl ArraySummationErrors {
        /// All the selectors of this enum.
        ///
        /// Note that the selectors might not be in the same order as the variants.
        /// No guarantees are made about the order of the selectors.
        ///
        /// Prefer using `SolInterface` methods instead.
        pub const SELECTORS: &'static [[u8; 4usize]] = &[
            [37u8, 47u8, 138u8, 14u8],
            [48u8, 92u8, 62u8, 147u8],
            [57u8, 141u8, 77u8, 50u8],
            [73u8, 63u8, 9u8, 196u8],
            [95u8, 111u8, 19u8, 44u8],
            [109u8, 134u8, 5u8, 219u8],
            [115u8, 118u8, 224u8, 162u8],
            [139u8, 170u8, 87u8, 159u8],
            [197u8, 42u8, 155u8, 211u8],
            [251u8, 187u8, 123u8, 43u8],
        ];
    }
    #[automatically_derived]
    impl alloy_sol_types::SolInterface for ArraySummationErrors {
        const NAME: &'static str = "ArraySummationErrors";
        const MIN_DATA_LENGTH: usize = 0usize;
        const COUNT: usize = 10usize;
        #[inline]
        fn selector(&self) -> [u8; 4] {
            match self {
                Self::FutureBlockNumber(_) => {
                    <FutureBlockNumber as alloy_sol_types::SolError>::SELECTOR
                }
                Self::InsufficientQuorumThreshold(_) => {
                    <InsufficientQuorumThreshold as alloy_sol_types::SolError>::SELECTOR
                }
                Self::InvalidArguments(_) => {
                    <InvalidArguments as alloy_sol_types::SolError>::SELECTOR
                }
                Self::InvalidConfiguration(_) => {
                    <InvalidConfiguration as alloy_sol_types::SolError>::SELECTOR
                }
                Self::InvalidOperation(_) => {
                    <InvalidOperation as alloy_sol_types::SolError>::SELECTOR
                }
                Self::InvalidSignature(_) => {
                    <InvalidSignature as alloy_sol_types::SolError>::SELECTOR
                }
                Self::InvalidStorageUpdates(_) => {
                    <InvalidStorageUpdates as alloy_sol_types::SolError>::SELECTOR
                }
                Self::InvalidTransitionIndex(_) => {
                    <InvalidTransitionIndex as alloy_sol_types::SolError>::SELECTOR
                }
                Self::RevertingContext(_) => {
                    <RevertingContext as alloy_sol_types::SolError>::SELECTOR
                }
                Self::StaleBlockNumber(_) => {
                    <StaleBlockNumber as alloy_sol_types::SolError>::SELECTOR
                }
            }
        }
        #[inline]
        fn selector_at(i: usize) -> ::core::option::Option<[u8; 4]> {
            Self::SELECTORS.get(i).copied()
        }
        #[inline]
        fn valid_selector(selector: [u8; 4]) -> bool {
            Self::SELECTORS.binary_search(&selector).is_ok()
        }
        #[inline]
        #[allow(non_snake_case)]
        fn abi_decode_raw(
            selector: [u8; 4],
            data: &[u8],
            validate: bool,
        ) -> alloy_sol_types::Result<Self> {
            static DECODE_SHIMS: &[fn(
                &[u8],
                bool,
            ) -> alloy_sol_types::Result<ArraySummationErrors>] = &[
                {
                    fn FutureBlockNumber(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationErrors> {
                        <FutureBlockNumber as alloy_sol_types::SolError>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationErrors::FutureBlockNumber)
                    }
                    FutureBlockNumber
                },
                {
                    fn StaleBlockNumber(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationErrors> {
                        <StaleBlockNumber as alloy_sol_types::SolError>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationErrors::StaleBlockNumber)
                    }
                    StaleBlockNumber
                },
                {
                    fn InvalidOperation(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationErrors> {
                        <InvalidOperation as alloy_sol_types::SolError>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationErrors::InvalidOperation)
                    }
                    InvalidOperation
                },
                {
                    fn RevertingContext(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationErrors> {
                        <RevertingContext as alloy_sol_types::SolError>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationErrors::RevertingContext)
                    }
                    RevertingContext
                },
                {
                    fn InvalidArguments(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationErrors> {
                        <InvalidArguments as alloy_sol_types::SolError>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationErrors::InvalidArguments)
                    }
                    InvalidArguments
                },
                {
                    fn InsufficientQuorumThreshold(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationErrors> {
                        <InsufficientQuorumThreshold as alloy_sol_types::SolError>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationErrors::InsufficientQuorumThreshold)
                    }
                    InsufficientQuorumThreshold
                },
                {
                    fn InvalidTransitionIndex(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationErrors> {
                        <InvalidTransitionIndex as alloy_sol_types::SolError>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationErrors::InvalidTransitionIndex)
                    }
                    InvalidTransitionIndex
                },
                {
                    fn InvalidSignature(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationErrors> {
                        <InvalidSignature as alloy_sol_types::SolError>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationErrors::InvalidSignature)
                    }
                    InvalidSignature
                },
                {
                    fn InvalidConfiguration(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationErrors> {
                        <InvalidConfiguration as alloy_sol_types::SolError>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationErrors::InvalidConfiguration)
                    }
                    InvalidConfiguration
                },
                {
                    fn InvalidStorageUpdates(
                        data: &[u8],
                        validate: bool,
                    ) -> alloy_sol_types::Result<ArraySummationErrors> {
                        <InvalidStorageUpdates as alloy_sol_types::SolError>::abi_decode_raw(
                                data,
                                validate,
                            )
                            .map(ArraySummationErrors::InvalidStorageUpdates)
                    }
                    InvalidStorageUpdates
                },
            ];
            let Ok(idx) = Self::SELECTORS.binary_search(&selector) else {
                return Err(
                    alloy_sol_types::Error::unknown_selector(
                        <Self as alloy_sol_types::SolInterface>::NAME,
                        selector,
                    ),
                );
            };
            DECODE_SHIMS[idx](data, validate)
        }
        #[inline]
        fn abi_encoded_size(&self) -> usize {
            match self {
                Self::FutureBlockNumber(inner) => {
                    <FutureBlockNumber as alloy_sol_types::SolError>::abi_encoded_size(
                        inner,
                    )
                }
                Self::InsufficientQuorumThreshold(inner) => {
                    <InsufficientQuorumThreshold as alloy_sol_types::SolError>::abi_encoded_size(
                        inner,
                    )
                }
                Self::InvalidArguments(inner) => {
                    <InvalidArguments as alloy_sol_types::SolError>::abi_encoded_size(
                        inner,
                    )
                }
                Self::InvalidConfiguration(inner) => {
                    <InvalidConfiguration as alloy_sol_types::SolError>::abi_encoded_size(
                        inner,
                    )
                }
                Self::InvalidOperation(inner) => {
                    <InvalidOperation as alloy_sol_types::SolError>::abi_encoded_size(
                        inner,
                    )
                }
                Self::InvalidSignature(inner) => {
                    <InvalidSignature as alloy_sol_types::SolError>::abi_encoded_size(
                        inner,
                    )
                }
                Self::InvalidStorageUpdates(inner) => {
                    <InvalidStorageUpdates as alloy_sol_types::SolError>::abi_encoded_size(
                        inner,
                    )
                }
                Self::InvalidTransitionIndex(inner) => {
                    <InvalidTransitionIndex as alloy_sol_types::SolError>::abi_encoded_size(
                        inner,
                    )
                }
                Self::RevertingContext(inner) => {
                    <RevertingContext as alloy_sol_types::SolError>::abi_encoded_size(
                        inner,
                    )
                }
                Self::StaleBlockNumber(inner) => {
                    <StaleBlockNumber as alloy_sol_types::SolError>::abi_encoded_size(
                        inner,
                    )
                }
            }
        }
        #[inline]
        fn abi_encode_raw(&self, out: &mut alloy_sol_types::private::Vec<u8>) {
            match self {
                Self::FutureBlockNumber(inner) => {
                    <FutureBlockNumber as alloy_sol_types::SolError>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::InsufficientQuorumThreshold(inner) => {
                    <InsufficientQuorumThreshold as alloy_sol_types::SolError>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::InvalidArguments(inner) => {
                    <InvalidArguments as alloy_sol_types::SolError>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::InvalidConfiguration(inner) => {
                    <InvalidConfiguration as alloy_sol_types::SolError>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::InvalidOperation(inner) => {
                    <InvalidOperation as alloy_sol_types::SolError>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::InvalidSignature(inner) => {
                    <InvalidSignature as alloy_sol_types::SolError>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::InvalidStorageUpdates(inner) => {
                    <InvalidStorageUpdates as alloy_sol_types::SolError>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::InvalidTransitionIndex(inner) => {
                    <InvalidTransitionIndex as alloy_sol_types::SolError>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::RevertingContext(inner) => {
                    <RevertingContext as alloy_sol_types::SolError>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
                Self::StaleBlockNumber(inner) => {
                    <StaleBlockNumber as alloy_sol_types::SolError>::abi_encode_raw(
                        inner,
                        out,
                    )
                }
            }
        }
    }
    ///Container for all the [`ArraySummation`](self) events.
    #[derive(serde::Serialize, serde::Deserialize)]
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub enum ArraySummationEvents {
        #[allow(missing_docs)]
        ArrayInitialized(ArrayInitialized),
        #[allow(missing_docs)]
        SumCalculated(SumCalculated),
    }
    #[automatically_derived]
    impl ArraySummationEvents {
        /// All the selectors of this enum.
        ///
        /// Note that the selectors might not be in the same order as the variants.
        /// No guarantees are made about the order of the selectors.
        ///
        /// Prefer using `SolInterface` methods instead.
        pub const SELECTORS: &'static [[u8; 32usize]] = &[
            [
                182u8, 11u8, 154u8, 134u8, 54u8, 169u8, 209u8, 247u8, 112u8, 115u8, 31u8,
                220u8, 72u8, 145u8, 43u8, 253u8, 172u8, 177u8, 216u8, 231u8, 102u8, 7u8,
                146u8, 201u8, 26u8, 5u8, 27u8, 221u8, 249u8, 214u8, 45u8, 77u8,
            ],
            [
                253u8, 61u8, 251u8, 179u8, 218u8, 6u8, 178u8, 113u8, 8u8, 72u8, 145u8,
                108u8, 101u8, 134u8, 106u8, 61u8, 14u8, 5u8, 0u8, 71u8, 64u8, 37u8,
                121u8, 166u8, 225u8, 113u8, 66u8, 97u8, 19u8, 124u8, 25u8, 198u8,
            ],
        ];
    }
    #[automatically_derived]
    impl alloy_sol_types::SolEventInterface for ArraySummationEvents {
        const NAME: &'static str = "ArraySummationEvents";
        const COUNT: usize = 2usize;
        fn decode_raw_log(
            topics: &[alloy_sol_types::Word],
            data: &[u8],
            validate: bool,
        ) -> alloy_sol_types::Result<Self> {
            match topics.first().copied() {
                Some(<ArrayInitialized as alloy_sol_types::SolEvent>::SIGNATURE_HASH) => {
                    <ArrayInitialized as alloy_sol_types::SolEvent>::decode_raw_log(
                            topics,
                            data,
                            validate,
                        )
                        .map(Self::ArrayInitialized)
                }
                Some(<SumCalculated as alloy_sol_types::SolEvent>::SIGNATURE_HASH) => {
                    <SumCalculated as alloy_sol_types::SolEvent>::decode_raw_log(
                            topics,
                            data,
                            validate,
                        )
                        .map(Self::SumCalculated)
                }
                _ => {
                    alloy_sol_types::private::Err(alloy_sol_types::Error::InvalidLog {
                        name: <Self as alloy_sol_types::SolEventInterface>::NAME,
                        log: alloy_sol_types::private::Box::new(
                            alloy_sol_types::private::LogData::new_unchecked(
                                topics.to_vec(),
                                data.to_vec().into(),
                            ),
                        ),
                    })
                }
            }
        }
    }
    #[automatically_derived]
    impl alloy_sol_types::private::IntoLogData for ArraySummationEvents {
        fn to_log_data(&self) -> alloy_sol_types::private::LogData {
            match self {
                Self::ArrayInitialized(inner) => {
                    alloy_sol_types::private::IntoLogData::to_log_data(inner)
                }
                Self::SumCalculated(inner) => {
                    alloy_sol_types::private::IntoLogData::to_log_data(inner)
                }
            }
        }
        fn into_log_data(self) -> alloy_sol_types::private::LogData {
            match self {
                Self::ArrayInitialized(inner) => {
                    alloy_sol_types::private::IntoLogData::into_log_data(inner)
                }
                Self::SumCalculated(inner) => {
                    alloy_sol_types::private::IntoLogData::into_log_data(inner)
                }
            }
        }
    }
    use alloy::contract as alloy_contract;
    /**Creates a new wrapper around an on-chain [`ArraySummation`](self) contract instance.

See the [wrapper's documentation](`ArraySummationInstance`) for more details.*/
    #[inline]
    pub const fn new<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    >(
        address: alloy_sol_types::private::Address,
        provider: P,
    ) -> ArraySummationInstance<T, P, N> {
        ArraySummationInstance::<T, P, N>::new(address, provider)
    }
    /**Deploys this contract using the given `provider` and constructor arguments, if any.

Returns a new instance of the contract, if the deployment was successful.

For more fine-grained control over the deployment process, use [`deploy_builder`] instead.*/
    #[inline]
    pub fn deploy<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    >(
        provider: P,
        _avsAddress: alloy::sol_types::private::Address,
        _blsSigChecker: alloy::sol_types::private::Address,
        _arraySize: alloy::sol_types::private::primitives::aliases::U256,
        _maxValue: alloy::sol_types::private::primitives::aliases::U256,
        _seed: alloy::sol_types::private::primitives::aliases::U256,
    ) -> impl ::core::future::Future<
        Output = alloy_contract::Result<ArraySummationInstance<T, P, N>>,
    > {
        ArraySummationInstance::<
            T,
            P,
            N,
        >::deploy(provider, _avsAddress, _blsSigChecker, _arraySize, _maxValue, _seed)
    }
    /**Creates a `RawCallBuilder` for deploying this contract using the given `provider`
and constructor arguments, if any.

This is a simple wrapper around creating a `RawCallBuilder` with the data set to
the bytecode concatenated with the constructor's ABI-encoded arguments.*/
    #[inline]
    pub fn deploy_builder<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    >(
        provider: P,
        _avsAddress: alloy::sol_types::private::Address,
        _blsSigChecker: alloy::sol_types::private::Address,
        _arraySize: alloy::sol_types::private::primitives::aliases::U256,
        _maxValue: alloy::sol_types::private::primitives::aliases::U256,
        _seed: alloy::sol_types::private::primitives::aliases::U256,
    ) -> alloy_contract::RawCallBuilder<T, P, N> {
        ArraySummationInstance::<
            T,
            P,
            N,
        >::deploy_builder(
            provider,
            _avsAddress,
            _blsSigChecker,
            _arraySize,
            _maxValue,
            _seed,
        )
    }
    /**A [`ArraySummation`](self) instance.

Contains type-safe methods for interacting with an on-chain instance of the
[`ArraySummation`](self) contract located at a given `address`, using a given
provider `P`.

If the contract bytecode is available (see the [`sol!`](alloy_sol_types::sol!)
documentation on how to provide it), the `deploy` and `deploy_builder` methods can
be used to deploy a new instance of the contract.

See the [module-level documentation](self) for all the available methods.*/
    #[derive(Clone)]
    pub struct ArraySummationInstance<T, P, N = alloy_contract::private::Ethereum> {
        address: alloy_sol_types::private::Address,
        provider: P,
        _network_transport: ::core::marker::PhantomData<(N, T)>,
    }
    #[automatically_derived]
    impl<T, P, N> ::core::fmt::Debug for ArraySummationInstance<T, P, N> {
        #[inline]
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            f.debug_tuple("ArraySummationInstance").field(&self.address).finish()
        }
    }
    /// Instantiation and getters/setters.
    #[automatically_derived]
    impl<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    > ArraySummationInstance<T, P, N> {
        /**Creates a new wrapper around an on-chain [`ArraySummation`](self) contract instance.

See the [wrapper's documentation](`ArraySummationInstance`) for more details.*/
        #[inline]
        pub const fn new(
            address: alloy_sol_types::private::Address,
            provider: P,
        ) -> Self {
            Self {
                address,
                provider,
                _network_transport: ::core::marker::PhantomData,
            }
        }
        /**Deploys this contract using the given `provider` and constructor arguments, if any.

Returns a new instance of the contract, if the deployment was successful.

For more fine-grained control over the deployment process, use [`deploy_builder`] instead.*/
        #[inline]
        pub async fn deploy(
            provider: P,
            _avsAddress: alloy::sol_types::private::Address,
            _blsSigChecker: alloy::sol_types::private::Address,
            _arraySize: alloy::sol_types::private::primitives::aliases::U256,
            _maxValue: alloy::sol_types::private::primitives::aliases::U256,
            _seed: alloy::sol_types::private::primitives::aliases::U256,
        ) -> alloy_contract::Result<ArraySummationInstance<T, P, N>> {
            let call_builder = Self::deploy_builder(
                provider,
                _avsAddress,
                _blsSigChecker,
                _arraySize,
                _maxValue,
                _seed,
            );
            let contract_address = call_builder.deploy().await?;
            Ok(Self::new(contract_address, call_builder.provider))
        }
        /**Creates a `RawCallBuilder` for deploying this contract using the given `provider`
and constructor arguments, if any.

This is a simple wrapper around creating a `RawCallBuilder` with the data set to
the bytecode concatenated with the constructor's ABI-encoded arguments.*/
        #[inline]
        pub fn deploy_builder(
            provider: P,
            _avsAddress: alloy::sol_types::private::Address,
            _blsSigChecker: alloy::sol_types::private::Address,
            _arraySize: alloy::sol_types::private::primitives::aliases::U256,
            _maxValue: alloy::sol_types::private::primitives::aliases::U256,
            _seed: alloy::sol_types::private::primitives::aliases::U256,
        ) -> alloy_contract::RawCallBuilder<T, P, N> {
            alloy_contract::RawCallBuilder::new_raw_deploy(
                provider,
                [
                    &BYTECODE[..],
                    &alloy_sol_types::SolConstructor::abi_encode(
                        &constructorCall {
                            _avsAddress,
                            _blsSigChecker,
                            _arraySize,
                            _maxValue,
                            _seed,
                        },
                    )[..],
                ]
                    .concat()
                    .into(),
            )
        }
        /// Returns a reference to the address.
        #[inline]
        pub const fn address(&self) -> &alloy_sol_types::private::Address {
            &self.address
        }
        /// Sets the address.
        #[inline]
        pub fn set_address(&mut self, address: alloy_sol_types::private::Address) {
            self.address = address;
        }
        /// Sets the address and returns `self`.
        pub fn at(mut self, address: alloy_sol_types::private::Address) -> Self {
            self.set_address(address);
            self
        }
        /// Returns a reference to the provider.
        #[inline]
        pub const fn provider(&self) -> &P {
            &self.provider
        }
    }
    impl<T, P: ::core::clone::Clone, N> ArraySummationInstance<T, &P, N> {
        /// Clones the provider and returns a new instance with the cloned provider.
        #[inline]
        pub fn with_cloned_provider(self) -> ArraySummationInstance<T, P, N> {
            ArraySummationInstance {
                address: self.address,
                provider: ::core::clone::Clone::clone(&self.provider),
                _network_transport: ::core::marker::PhantomData,
            }
        }
    }
    /// Function calls.
    #[automatically_derived]
    impl<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    > ArraySummationInstance<T, P, N> {
        /// Creates a new call builder using this contract instance's provider and address.
        ///
        /// Note that the call can be any function call, not just those defined in this
        /// contract. Prefer using the other methods for building type-safe contract calls.
        pub fn call_builder<C: alloy_sol_types::SolCall>(
            &self,
            call: &C,
        ) -> alloy_contract::SolCallBuilder<T, &P, C, N> {
            alloy_contract::SolCallBuilder::new_sol(&self.provider, &self.address, call)
        }
        ///Creates a new call builder for the [`BLOCK_STALE_MEASURE`] function.
        pub fn BLOCK_STALE_MEASURE(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, BLOCK_STALE_MEASURECall, N> {
            self.call_builder(&BLOCK_STALE_MEASURECall {})
        }
        ///Creates a new call builder for the [`QUORUM_THRESHOLD`] function.
        pub fn QUORUM_THRESHOLD(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, QUORUM_THRESHOLDCall, N> {
            self.call_builder(&QUORUM_THRESHOLDCall {})
        }
        ///Creates a new call builder for the [`THRESHOLD_DENOMINATOR`] function.
        pub fn THRESHOLD_DENOMINATOR(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, THRESHOLD_DENOMINATORCall, N> {
            self.call_builder(&THRESHOLD_DENOMINATORCall {})
        }
        ///Creates a new call builder for the [`arraySize`] function.
        pub fn arraySize(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, arraySizeCall, N> {
            self.call_builder(&arraySizeCall {})
        }
        ///Creates a new call builder for the [`avsAddress`] function.
        pub fn avsAddress(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, avsAddressCall, N> {
            self.call_builder(&avsAddressCall {})
        }
        ///Creates a new call builder for the [`blsSignatureChecker`] function.
        pub fn blsSignatureChecker(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, blsSignatureCheckerCall, N> {
            self.call_builder(&blsSignatureCheckerCall {})
        }
        ///Creates a new call builder for the [`currentSum`] function.
        pub fn currentSum(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, currentSumCall, N> {
            self.call_builder(&currentSumCall {})
        }
        ///Creates a new call builder for the [`getArrayElement`] function.
        pub fn getArrayElement(
            &self,
            index: alloy::sol_types::private::primitives::aliases::U256,
        ) -> alloy_contract::SolCallBuilder<T, &P, getArrayElementCall, N> {
            self.call_builder(&getArrayElementCall { index })
        }
        ///Creates a new call builder for the [`getArrayLength`] function.
        pub fn getArrayLength(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, getArrayLengthCall, N> {
            self.call_builder(&getArrayLengthCall {})
        }
        ///Creates a new call builder for the [`getFullArray`] function.
        pub fn getFullArray(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, getFullArrayCall, N> {
            self.call_builder(&getFullArrayCall {})
        }
        ///Creates a new call builder for the [`maxValue`] function.
        pub fn maxValue(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, maxValueCall, N> {
            self.call_builder(&maxValueCall {})
        }
        ///Creates a new call builder for the [`namespace`] function.
        pub fn namespace(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, namespaceCall, N> {
            self.call_builder(&namespaceCall {})
        }
        ///Creates a new call builder for the [`resetArray`] function.
        pub fn resetArray(
            &self,
            _seed: alloy::sol_types::private::primitives::aliases::U256,
        ) -> alloy_contract::SolCallBuilder<T, &P, resetArrayCall, N> {
            self.call_builder(&resetArrayCall { _seed })
        }
        ///Creates a new call builder for the [`setArrayElement`] function.
        pub fn setArrayElement(
            &self,
            index: alloy::sol_types::private::primitives::aliases::U256,
            newValue: alloy::sol_types::private::primitives::aliases::U256,
        ) -> alloy_contract::SolCallBuilder<T, &P, setArrayElementCall, N> {
            self.call_builder(
                &setArrayElementCall {
                    index,
                    newValue,
                },
            )
        }
        ///Creates a new call builder for the [`stateTransitionCount`] function.
        pub fn stateTransitionCount(
            &self,
        ) -> alloy_contract::SolCallBuilder<T, &P, stateTransitionCountCall, N> {
            self.call_builder(&stateTransitionCountCall {})
        }
        ///Creates a new call builder for the [`sum`] function.
        pub fn sum(
            &self,
            indexes: alloy::sol_types::private::Vec<
                alloy::sol_types::private::primitives::aliases::U256,
            >,
        ) -> alloy_contract::SolCallBuilder<T, &P, sumCall, N> {
            self.call_builder(&sumCall { indexes })
        }
        ///Creates a new call builder for the [`values`] function.
        pub fn values(
            &self,
            _0: alloy::sol_types::private::primitives::aliases::U256,
        ) -> alloy_contract::SolCallBuilder<T, &P, valuesCall, N> {
            self.call_builder(&valuesCall { _0 })
        }
        ///Creates a new call builder for the [`verifyAndUpdate`] function.
        pub fn verifyAndUpdate(
            &self,
            msgHash: alloy::sol_types::private::FixedBytes<32>,
            quorumNumbers: alloy::sol_types::private::Bytes,
            referenceBlockNumber: u32,
            storageUpdates: alloy::sol_types::private::Bytes,
            transitionIndex: alloy::sol_types::private::primitives::aliases::U256,
            targetAddr: alloy::sol_types::private::Address,
            targetFunction: alloy::sol_types::private::FixedBytes<4>,
            nonSignerStakesAndSignature: <IBLSSignatureCheckerTypes::NonSignerStakesAndSignature as alloy::sol_types::SolType>::RustType,
        ) -> alloy_contract::SolCallBuilder<T, &P, verifyAndUpdateCall, N> {
            self.call_builder(
                &verifyAndUpdateCall {
                    msgHash,
                    quorumNumbers,
                    referenceBlockNumber,
                    storageUpdates,
                    transitionIndex,
                    targetAddr,
                    targetFunction,
                    nonSignerStakesAndSignature,
                },
            )
        }
    }
    /// Event filters.
    #[automatically_derived]
    impl<
        T: alloy_contract::private::Transport + ::core::clone::Clone,
        P: alloy_contract::private::Provider<T, N>,
        N: alloy_contract::private::Network,
    > ArraySummationInstance<T, P, N> {
        /// Creates a new event filter using this contract instance's provider and address.
        ///
        /// Note that the type can be any event, not just those defined in this contract.
        /// Prefer using the other methods for building type-safe event filters.
        pub fn event_filter<E: alloy_sol_types::SolEvent>(
            &self,
        ) -> alloy_contract::Event<T, &P, E, N> {
            alloy_contract::Event::new_sol(&self.provider, &self.address)
        }
        ///Creates a new event filter for the [`ArrayInitialized`] event.
        pub fn ArrayInitialized_filter(
            &self,
        ) -> alloy_contract::Event<T, &P, ArrayInitialized, N> {
            self.event_filter::<ArrayInitialized>()
        }
        ///Creates a new event filter for the [`SumCalculated`] event.
        pub fn SumCalculated_filter(
            &self,
        ) -> alloy_contract::Event<T, &P, SumCalculated, N> {
            self.event_filter::<SumCalculated>()
        }
    }
}
