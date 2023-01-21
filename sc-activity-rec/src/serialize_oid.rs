use git2::Oid;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(crate) fn serialize<S>(oid: &Oid, ser: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    oid.to_string().serialize(ser)
}

#[allow(unused)]
pub(crate) fn deserialize<'de, D>(deser: D) -> Result<Oid, D::Error>
where
    D: Deserializer<'de>,
{
    Oid::from_str(&String::deserialize(deser)?)
        .map_err(|err| Error::custom(format!("Git: {}", err.message())))
}

pub(crate) mod option {
    use super::*;

    pub(crate) fn serialize<S>(oid: &Option<Oid>, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        oid.map(|oidstr| oidstr.to_string()).serialize(ser)
    }

    #[allow(unused)]
    pub(crate) fn deserialize<'de, D>(deser: D) -> Result<Option<Oid>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<String>::deserialize(deser)?
            .map(|thestr| {
                Oid::from_str(&thestr)
                    .map_err(|err| Error::custom(format!("Git: {}", err.message())))
            })
            .transpose()
    }
}
