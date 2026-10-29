//! The data state (`setData` / `getData`, `RState`).
//!
//! It is a map by type that travels in `Ctx` and is copied when forking: what one branch writes the other does not see.
//! The mutable cells (`RState`) are values of the map that hold an `Arc<Mutex<..>>`: when the map is copied
//! both branches point to the same cell, and what one writes the other sees. A new cell (`new_rstate`) is only
//! seen by the branch that creates it and by those born after it.

use super::*;

impl Ctx {
    pub(crate) fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.data.get(&TypeId::of::<T>()).map(|a| a.clone().downcast::<T>().unwrap())
    }

    pub(crate) fn set<T: Any + Send + Sync>(&mut self, x: T) {
        Arc::make_mut(&mut self.data).insert(TypeId::of::<T>(), Arc::new(x));
    }

    pub(crate) fn del<T: Any + Send + Sync>(&mut self) {
        Arc::make_mut(&mut self.data).remove(&TypeId::of::<T>());
    }
}

/// Stores a value in the state; it replaces the previous one of the same type. Only this thread and those born from it see it.
pub fn set_data<T: Clone + Send + Sync + 'static>(x: T) -> Trans<()> {
    Trans::new(move |ctx| {
        ctx.set(x.clone());
        Some(())
    })
}

/// The value in the state of that type, if there is one
pub fn get_data<T: Send + Sync + 'static>() -> Trans<Option<Arc<T>>> {
    Trans::new(|ctx| Some(ctx.get::<T>()))
}

pub fn del_data<T: Send + Sync + 'static>() -> Trans<()> {
    Trans::new(|ctx| {
        ctx.del::<T>();
        Some(())
    })
}

/// A mutable cell of the state (`newtype Ref a = Ref (IORef (Maybe a))`)
pub struct Ref<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Ref<T> {
    fn clone(&self) -> Self {
        Ref(self.0.clone())
    }
}

impl<T> Ref<T> {
    pub fn set(&self, x: T) {
        *self.0.lock().unwrap() = Some(x);
    }
    pub fn get(&self) -> Option<T>
    where
        T: Clone,
    {
        self.0.lock().unwrap().clone()
    }
}

/// Creates a cell with `x`, stores it in the state (one per type) and returns it. It always creates a new one.
pub fn new_rstate<T: Clone + Send + Sync + 'static>(x: T) -> Trans<Ref<T>> {
    Trans::new(move |ctx| {
        let r = Ref(Arc::new(Mutex::new(Some(x.clone()))));
        ctx.set(r.clone());
        Some(r)
    })
}

/// Writes in the cell of that type; if it does not exist it creates it
pub fn set_rstate<T: Clone + Send + Sync + 'static>(x: T) -> Trans<()> {
    Trans::new(move |ctx| {
        match ctx.get::<Ref<T>>() {
            Some(r) => r.set(x.clone()),
            None => ctx.set(Ref(Arc::new(Mutex::new(Some(x.clone()))))),
        }
        Some(())
    })
}

/// The value of the cell of that type. If it does not exist, or was deleted, the computation stops (`empty`).
pub fn get_rstate<T: Clone + Send + Sync + 'static>() -> Trans<T> {
    Trans::new(|ctx| {
        let r = ctx.get::<Ref<T>>()?;
        let v = r.get();
        if v.is_none() {
            ctx.del::<Ref<T>>();
        }
        v
    })
}

/// Changes the value of the cell of that type with `f`; if it does not exist, it does nothing
pub fn modify_rstate<T: Clone + Send + Sync + 'static>(f: impl Fn(T) -> T + Send + Sync + 'static) -> Trans<()> {
    Trans::new(move |ctx| {
        if let Some(r) = ctx.get::<Ref<T>>() {
            let mut v = r.0.lock().unwrap();
            *v = v.take().map(&f);
        }
        Some(())
    })
}

/// Deletes the cell of that type, for all the branches that share it
pub fn del_rstate<T: Clone + Send + Sync + 'static>() -> Trans<()> {
    Trans::new(|ctx| {
        if let Some(r) = ctx.get::<Ref<T>>() {
            *r.0.lock().unwrap() = None;
            ctx.del::<Ref<T>>();
        }
        Some(())
    })
}

// ---------------------------------------------------------------------------------------------------------------
// the rest of the operations on the state of `Transient.Base`

/// The data of that type; if it does not exist, the computation stops (`getSData`, `tryState`)
pub fn get_sdata<T: Send + Sync + 'static>() -> Trans<Arc<T>> {
    Trans::new(|ctx| ctx.get::<T>())
}

/// Another name for `get_sdata` (`tryState`)
pub fn try_state<T: Send + Sync + 'static>() -> Trans<Arc<T>> {
    get_sdata()
}

/// The data of that type, or the default value if it does not exist (`getState`)
pub fn get_state<T: Default + Send + Sync + 'static>() -> Trans<Arc<T>> {
    Trans::new(|ctx| Some(ctx.get::<T>().unwrap_or_default()))
}

/// Another name for `set_data` (`setState`)
pub fn set_state<T: Clone + Send + Sync + 'static>(x: T) -> Trans<()> {
    set_data(x)
}

/// Another name for `del_data` (`delState`)
pub fn del_state<T: Send + Sync + 'static>() -> Trans<()> {
    del_data::<T>()
}

/// Changes the data of that type with `f`, which receives the current value if there is one. If it returns `None` it is deleted
/// (`modifyData`).
pub fn modify_data<T: Clone + Send + Sync + 'static>(f: impl Fn(Option<T>) -> Option<T> + Send + Sync + 'static) -> Trans<()> {
    Trans::new(move |ctx| {
        match f(ctx.get::<T>().map(|v| (*v).clone())) {
            Some(v) => ctx.set(v),
            None => ctx.del::<T>(),
        }
        Some(())
    })
}

/// If the data exists it changes it with `f`; otherwise, it stores `v`. It returns the value that is left (`modifyData'`):
///
/// * with no previous data: `modify_data_with(|h| h + " world", "hello new")` stores and returns `"hello new"`
/// * with `"hello"`: stores and returns `"hello world"`
pub fn modify_data_with<T: Clone + Send + Sync + 'static>(f: impl Fn(T) -> T + Send + Sync + 'static, v: T) -> Trans<T> {
    Trans::new(move |ctx| {
        let new = match ctx.get::<T>() {
            Some(old) => f((*old).clone()),
            None => v.clone(),
        };
        ctx.set(new.clone());
        Some(new)
    })
}

/// Another name for `modify_data` (`modifyState`)
pub fn modify_state<T: Clone + Send + Sync + 'static>(f: impl Fn(Option<T>) -> Option<T> + Send + Sync + 'static) -> Trans<()> {
    modify_data(f)
}

/// Another name for `modify_data_with` (`modifyState'`)
pub fn modify_state_with<T: Clone + Send + Sync + 'static>(f: impl Fn(T) -> T + Send + Sync + 'static, v: T) -> Trans<T> {
    modify_data_with(f, v)
}

// ---- indexed data: a map by type of key and of value

type Index<K, A> = std::collections::BTreeMap<K, A>;

/// The value of the key `k` in the pure map of that type, which is copied when forking (`getIndexData`)
pub fn get_index_data<K, A>(k: K) -> Trans<Option<A>>
where
    K: Ord + Clone + Send + Sync + 'static,
    A: Clone + Send + Sync + 'static,
{
    Trans::new(move |ctx| Some(ctx.get::<Index<K, A>>().and_then(|m| m.get(&k).cloned())))
}

/// Stores `a` in the key `k` of the pure map of that type (`setIndexData`)
pub fn set_index_data<K, A>(k: K, a: A) -> Trans<()>
where
    K: Ord + Clone + Send + Sync + 'static,
    A: Clone + Send + Sync + 'static,
{
    Trans::new(move |ctx| {
        let mut m = ctx.get::<Index<K, A>>().map(|m| (*m).clone()).unwrap_or_default();
        m.insert(k.clone(), a.clone());
        ctx.set(m);
        Some(())
    })
}

/// The value of the key `k` in the mutable map of that type, shared by the branches that inherit it (`getIndexRData`)
pub fn get_index_rdata<K, A>(k: K) -> Trans<Option<A>>
where
    K: Ord + Clone + Send + Sync + 'static,
    A: Clone + Send + Sync + 'static,
{
    Trans::new(move |ctx| Some(ctx.get::<Ref<Index<K, A>>>().and_then(|r| r.get()).and_then(|m| m.get(&k).cloned())))
}

/// Stores `a` in the key `k` of the mutable map of that type, creating it if it does not exist (`setIndexRData`)
pub fn set_index_rdata<K, A>(k: K, a: A) -> Trans<()>
where
    K: Ord + Clone + Send + Sync + 'static,
    A: Clone + Send + Sync + 'static,
{
    Trans::new(move |ctx| {
        let cell = match ctx.get::<Ref<Index<K, A>>>() {
            Some(c) => (*c).clone(),
            None => {
                let c = Ref(Arc::new(Mutex::new(Some(Index::<K, A>::new()))));
                ctx.set(c.clone());
                c
            }
        };
        let mut g = cell.0.lock().unwrap();
        g.get_or_insert_with(Default::default).insert(k.clone(), a.clone());
        Some(())
    })
}

/// Applies `f` to the value of the key `i` of the mutable map; if there is no value, it stores `n` (`withIndexRData`)
pub fn with_index_rdata<K, A>(i: K, f: impl Fn(A) -> A + Send + Sync + 'static, n: A) -> Trans<()>
where
    K: Ord + Clone + Send + Sync + 'static,
    A: Clone + Send + Sync + 'static,
{
    let f = Arc::new(f);
    Trans::new(move |ctx| {
        let current = get_index_rdata::<K, A>(i.clone()).run(ctx)?;
        let v = match current {
            Some(v) => f(v),
            None => n.clone(),
        };
        set_index_rdata(i.clone(), v).run(ctx)
    })
}
