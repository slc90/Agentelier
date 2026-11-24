use super::read;
use agentelier_bridge_connection::ecs_access::EcsError;
use bevy::prelude::{Entity, Name};
use bevy::reflect::{
    PartialReflect, TypeRegistry,
    serde::{ReflectSerializerProcessor, TypedReflectSerializer},
};
use serde::{Serializer, ser::Error as _};
use serde_json::Value;
use std::{cell::Cell, io};

const MAX_VALUE_NODES: usize = 1024;
const MAX_VALUE_BYTES: usize = 32 * 1024;

pub(super) fn encode(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
) -> Result<Value, EcsError> {
    let budget = NodeBudget(Cell::new(MAX_VALUE_NODES));
    let serializer = TypedReflectSerializer::with_processor(value, registry, &budget);
    let mut output = BoundedOutput::default();
    serde_json::to_writer(&mut output, &serializer).map_err(|error| {
        if output.exhausted || budget.0.get() == 0 {
            EcsError::new("ecs_result_limit", "反射值超过 ECS 大小限制")
        } else {
            EcsError::new(
                "unsupported_type",
                format!("{}: {error}", value.reflect_type_path()),
            )
        }
    })?;
    serde_json::from_slice(&output.bytes).map_err(|error| {
        EcsError::new(
            "unsupported_type",
            format!("{}: {error}", value.reflect_type_path()),
        )
    })
}

struct NodeBudget(Cell<usize>);

impl ReflectSerializerProcessor for NodeBudget {
    fn try_serialize<S: Serializer>(
        &self,
        value: &dyn PartialReflect,
        _registry: &TypeRegistry,
        serializer: S,
    ) -> Result<Result<S::Ok, S>, S::Error> {
        let left = self
            .0
            .get()
            .checked_sub(1)
            .ok_or_else(|| S::Error::custom("ECS 值节点数量超过限制"))?;
        self.0.set(left);
        if let Some(name) = value.try_downcast_ref::<Name>() {
            return serializer.serialize_str(name.as_str()).map(Ok);
        }
        if let Some(entity) = value.try_downcast_ref::<Entity>() {
            return serializer.serialize_str(&read::token(*entity)).map(Ok);
        }
        if value
            .try_downcast_ref::<f32>()
            .is_some_and(|value| !value.is_finite())
            || value
                .try_downcast_ref::<f64>()
                .is_some_and(|value| !value.is_finite())
        {
            return Err(S::Error::custom("反射浮点数必须为有限值"));
        }
        Ok(Err(serializer))
    }
}

#[derive(Default)]
struct BoundedOutput {
    bytes: Vec<u8>,
    exhausted: bool,
}

impl io::Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_VALUE_BYTES.saturating_sub(self.bytes.len()) {
            self.exhausted = true;
            return Err(io::Error::other("反射值超过 ECS 字节限制"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
