pub use inject_macros::inject;

#[doc(hidden)]
pub mod __private {
    pub use tokio;
}

#[macro_export]
macro_rules! install_injectable_container {
    ($state:ty) => {
        $crate::__private::tokio::task_local! { static SCOPED: $state; }
        static GLOBAL: std::sync::OnceLock<$state> = std::sync::OnceLock::new();

        pub fn init_container(state: $state) {
            if GLOBAL.set(state).is_err() {
                panic!("container already initialized");
            }
        }
        pub async fn scope<F: Future>(state: $state, fut: F) -> F::Output {
            SCOPED.scope(state, fut).await
        }
        pub fn current() -> $state {
            SCOPED
                .try_with(|s| s.clone())
                .ok()
                .or_else(|| GLOBAL.get().cloned())
                .expect("no container: call init_container() at startup or wrap tests in scope()")
        }
    };
}

#[macro_export]
macro_rules! register_injectable {
    ($name:ident($inner:ty), $field:ident) => {
        #[derive(Clone)]
        pub struct $name(pub $inner);
        impl std::ops::Deref for $name {
            type Target = $inner;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        impl std::ops::DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
        impl $crate::Resolve for $name {
            fn resolve() -> Self {
                $name(current().$field.clone())
            }
        }
    };
}

pub trait Resolve: Sized {
    fn resolve() -> Self;
}
