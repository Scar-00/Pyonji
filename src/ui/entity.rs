use std::{
    any::{Any, TypeId},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use slotmap::{KeyData, SecondaryMap};

use crate::ui::{AppContext, context::Context};

slotmap::new_key_type! {
    pub struct EntityId;
}

impl From<u64> for EntityId {
    fn from(value: u64) -> Self {
        Self(KeyData::from_ffi(value))
    }
}

pub struct EntityMap {
    entities: SecondaryMap<EntityId, Box<dyn Any + 'static>>,
    count: u64,
}

impl EntityMap {
    pub fn new() -> Self {
        Self {
            entities: SecondaryMap::default(),
            count: 0,
        }
    }

    pub fn reserve<T>(&mut self) -> Slot<T> {
        let id = self.count;
        self.count += 1;
        Slot {
            handle: Entity::new(id.into()),
        }
    }

    pub fn insert<T: 'static>(&mut self, slot: Slot<T>, value: T) -> Entity<T> {
        let handle = slot.handle.clone();
        self.entities.insert(slot.handle.id, Box::new(value));
        handle
    }

    #[track_caller]
    pub fn lease<T: 'static>(&mut self, id: EntityId) -> Lease<T> {
        //self.entities.get_mut(id).expect("entity in map").as_mut().downcast_mut::<T>().unwrap()
        let Some(ent_data) = self.entities.remove(id) else {
            panic!("cannot borrow: entity already borrowed")
        };
        Lease {
            entity_data: ent_data,
            id,
            _data: PhantomData {},
        }
    }

    pub fn end_lease<T: 'static>(&mut self, lease: Lease<T>) {
        self.entities.insert(lease.id, lease.entity_data);
    }
}

pub struct Lease<T: 'static> {
    entity_data: Box<dyn Any + 'static>,
    id: EntityId,
    _data: PhantomData<T>,
}

impl<T: 'static> Deref for Lease<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.entity_data.downcast_ref().unwrap()
    }
}

impl<T: 'static> DerefMut for Lease<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.entity_data.downcast_mut().unwrap()
    }
}

pub struct Slot<T: 'static> {
    pub handle: Entity<T>,
}

pub struct Entity<T: 'static> {
    pub id: EntityId,
    type_id: TypeId,
    _data: PhantomData<T>,
}

impl<T: 'static> Clone for Entity<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            type_id: self.type_id.clone(),
            _data: PhantomData {},
        }
    }
}

impl<T: 'static> Entity<T> {
    fn new(id: EntityId) -> Self {
        Self {
            id,
            type_id: TypeId::of::<T>(),
            _data: PhantomData {},
        }
    }

    #[track_caller]
    pub fn update<R>(
        &self,
        cx: &mut impl AppContext,
        update: impl Fn(&mut T, &mut Context<T>) -> R,
    ) -> R {
        cx.update_entity(self, update)
    }
}
