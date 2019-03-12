use pdsl_core::storage;
use pdsl_lang::contract;

contract! {
    /// A simple contract that has a value that can be
    /// incremented, returned and compared.
    struct Incrementer {
        /// The internal value.
        value: storage::Value<u32>,
    }

    impl Deploy for Incrementer {
        /// Automatically called when the contract is deployed.
        fn deploy(&mut self, init_value: u32) {
            self.value.set(init_value)
        }
    }

    impl Incrementer {
        /// Increments the internal counter.
        pub(external) fn inc(&mut self, by: u32) {
            self.value += by
        }

        /// Returns the internal counter.
        pub(external) fn get(&self) -> u32 {
            *self.value
        }

        /// Returns `true` if `x` is greater than the internal value.
        pub(external) fn compare(&self, x: u32) -> bool {
            x > *self.value
        }
    }
}
