// Copyright © 2019 Intel Corporation
// Copyright © 2023 Rivos, Inc.
// Copyright 2024 Amazon.com, Inc. or its affiliates. All Rights Reserved.
//
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/acpi-tables/src/aml.rs (trimmed: this port
// builds a serial + i8042 + _S5 DSDT only, so the AML subset is `Aml`,
// `Name`, `Package`, `Byte`/`Word`/`DWord`/`QWord`, `Path`, `Device`,
// `Method`, `ResourceTemplate`, `Io`, `Interrupt`, `EisaName`, `Usize`,
// `AmlStr`/`AmlString` and `Return`; `Scope`, `Field`, `OpRegion`,
// `AddressSpace`, `Memory32Fixed`, the operator/term nodes and the
// `displaydoc` dependency are dropped, the latter's `Display` derivation
// replaced with explicit `#[error]` strings. The PkgLength encoding is
// upstream's, unchanged).

#![allow(missing_debug_implementations)]

#[derive(Debug, Clone, thiserror::Error)]
pub enum AmlError {
    /// Aml Path is empty
    #[error("AML path is empty")]
    NameEmpty,
    /// Invalid name part length
    #[error("invalid AML name part length")]
    InvalidPartLength,
    /// Invalid address range
    #[error("invalid AML address range")]
    AddressRange,
}

pub trait Aml {
    fn append_aml_bytes(&self, _v: &mut Vec<u8>) -> Result<(), AmlError>;

    fn to_aml_bytes(&self) -> Result<Vec<u8>, AmlError> {
        let mut v = Vec::new();
        self.append_aml_bytes(&mut v)?;
        Ok(v)
    }
}

pub struct Path {
    root: bool,
    name_parts: Vec<[u8; 4]>,
}

impl Aml for Path {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        if self.root {
            bytes.push(b'\\');
        }

        match self.name_parts.len() {
            0 => return Err(AmlError::NameEmpty),
            1 => {}
            2 => {
                bytes.push(0x2e); // DualNamePrefix
            }
            n => {
                bytes.push(0x2f); // MultiNamePrefix
                bytes.push(n.try_into().unwrap());
            }
        };

        for part in &self.name_parts {
            bytes.extend_from_slice(part);
        }

        Ok(())
    }
}

impl Path {
    pub fn new(name: &str) -> Result<Self, AmlError> {
        let root = name.starts_with('\\');
        let offset = root.into();
        let mut name_parts = Vec::new();
        for part in name[offset..].split('.') {
            if part.len() != 4 {
                return Err(AmlError::InvalidPartLength);
            }
            let mut name_part = [0u8; 4];
            name_part.copy_from_slice(part.as_bytes());
            name_parts.push(name_part);
        }

        Ok(Path { root, name_parts })
    }
}

impl TryFrom<&str> for Path {
    type Error = AmlError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Path::new(s)
    }
}

pub type Byte = u8;

impl Aml for Byte {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        bytes.push(0x0a); // BytePrefix
        bytes.push(*self);
        Ok(())
    }
}

pub type Word = u16;

impl Aml for Word {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        bytes.push(0x0b); // WordPrefix
        bytes.extend_from_slice(&self.to_le_bytes());
        Ok(())
    }
}

pub type DWord = u32;

impl Aml for DWord {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        bytes.push(0x0c); // DWordPrefix
        bytes.extend_from_slice(&self.to_le_bytes());
        Ok(())
    }
}

pub type QWord = u64;

impl Aml for QWord {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        bytes.push(0x0e); // QWordPrefix
        bytes.extend_from_slice(&self.to_le_bytes());
        Ok(())
    }
}

pub struct Name {
    bytes: Vec<u8>,
}

impl Aml for Name {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        bytes.extend_from_slice(&self.bytes);
        Ok(())
    }
}

impl Name {
    pub fn new(path: Path, inner: &dyn Aml) -> Result<Self, AmlError> {
        let mut bytes = vec![0x08]; // NameOp
        path.append_aml_bytes(&mut bytes)?;
        inner.append_aml_bytes(&mut bytes)?;
        Ok(Name { bytes })
    }
}

pub struct Package<'a> {
    children: Vec<&'a dyn Aml>,
}

impl Aml for Package<'_> {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        let mut tmp = vec![self.children.len().try_into().unwrap()];
        for child in &self.children {
            child.append_aml_bytes(&mut tmp)?;
        }

        let pkg_length = create_pkg_length(&tmp, true);

        bytes.push(0x12); // PackageOp
        bytes.extend_from_slice(&pkg_length);
        bytes.extend_from_slice(&tmp);
        Ok(())
    }
}

impl<'a> Package<'a> {
    pub fn new(children: Vec<&'a dyn Aml>) -> Self {
        Package { children }
    }
}

// From the ACPI spec for PkgLength:
//
// "The high 2 bits of the first byte reveal how many follow bytes are in the PkgLength. If the
// PkgLength has only one byte, bit 0 through 5 are used to encode the package length (in other
// words, values 0-63). If the package length value is more than 63, more than one byte must be
// used for the encoding in which case bit 4 and 5 of the PkgLeadByte are reserved and must be zero.
// If the multiple bytes encoding is used, bits 0-3 of the PkgLeadByte become the least significant
// 4 bits of the resulting package length value. The next ByteData will become the next least
// significant 8 bits of the resulting value and so on, up to 3 ByteData bytes. Thus, the maximum
// package length is 2**28."
//

// Also used for NamedField but in that case the length is not included in itself
fn create_pkg_length(data: &[u8], include_self: bool) -> Vec<u8> {
    let mut result = Vec::new();

    // PkgLength is inclusive and includes the length bytes
    let length_length = if data.len() < (2usize.pow(6) - 1) {
        1
    } else if data.len() < (2usize.pow(12) - 2) {
        2
    } else if data.len() < (2usize.pow(20) - 3) {
        3
    } else {
        4
    };

    let length = data.len() + if include_self { length_length } else { 0 };

    match length_length {
        1 => result.push(length.try_into().unwrap()),
        2 => {
            result.push((1u8 << 6) | TryInto::<u8>::try_into(length & 0xf).unwrap());
            result.push(TryInto::<u8>::try_into(length >> 4).unwrap())
        }
        3 => {
            result.push((2u8 << 6) | TryInto::<u8>::try_into(length & 0xf).unwrap());
            result.push(((length >> 4) & 0xff).try_into().unwrap());
            result.push(((length >> 12) & 0xff).try_into().unwrap());
        }
        _ => {
            result.push((3u8 << 6) | TryInto::<u8>::try_into(length & 0xf).unwrap());
            result.push(((length >> 4) & 0xff).try_into().unwrap());
            result.push(((length >> 12) & 0xff).try_into().unwrap());
            result.push(((length >> 20) & 0xff).try_into().unwrap());
        }
    }

    result
}

pub struct EisaName {
    value: DWord,
}

impl EisaName {
    pub fn new(name: &str) -> Result<Self, AmlError> {
        if name.len() != 7 {
            return Err(AmlError::InvalidPartLength);
        }

        let data = name.as_bytes();

        let value: u32 = ((u32::from(data[0] - 0x40) << 26)
            | (u32::from(data[1] - 0x40) << 21)
            | (u32::from(data[2] - 0x40) << 16)
            | (name.chars().nth(3).unwrap().to_digit(16).unwrap() << 12)
            | (name.chars().nth(4).unwrap().to_digit(16).unwrap() << 8)
            | (name.chars().nth(5).unwrap().to_digit(16).unwrap() << 4)
            | name.chars().nth(6).unwrap().to_digit(16).unwrap())
        .swap_bytes();

        Ok(EisaName { value })
    }
}

impl Aml for EisaName {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        self.value.append_aml_bytes(bytes)
    }
}

pub type Usize = usize;

impl Aml for Usize {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        if *self <= u8::MAX.into() {
            TryInto::<u8>::try_into(*self)
                .unwrap()
                .append_aml_bytes(bytes)
        } else if *self <= u16::MAX.into() {
            TryInto::<u16>::try_into(*self)
                .unwrap()
                .append_aml_bytes(bytes)
        } else if *self <= u32::MAX as usize {
            TryInto::<u32>::try_into(*self)
                .unwrap()
                .append_aml_bytes(bytes)
        } else {
            TryInto::<u64>::try_into(*self)
                .unwrap()
                .append_aml_bytes(bytes)
        }
    }
}

fn append_aml_string(v: &str, bytes: &mut Vec<u8>) {
    bytes.push(0x0D); // String Op
    bytes.extend_from_slice(v.as_bytes());
    bytes.push(0x0); // NullChar
}

pub type AmlStr = &'static str;

impl Aml for AmlStr {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        append_aml_string(self, bytes);
        Ok(())
    }
}

pub type AmlString = String;

impl Aml for AmlString {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        append_aml_string(self, bytes);
        Ok(())
    }
}

pub struct ResourceTemplate<'a> {
    children: Vec<&'a dyn Aml>,
}

impl Aml for ResourceTemplate<'_> {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        let mut tmp = Vec::new();
        // Add buffer data
        for child in &self.children {
            child.append_aml_bytes(&mut tmp)?;
        }

        // Mark with end and mark checksum as as always valid
        tmp.push(0x79); // EndTag
        tmp.push(0); // zero checksum byte

        // Buffer length is an encoded integer including buffer data
        // and EndTag and checksum byte
        let mut buffer_length = tmp.len().to_aml_bytes()?;
        buffer_length.reverse();
        for byte in buffer_length {
            tmp.insert(0, byte);
        }

        // PkgLength is everything else
        let pkg_length = create_pkg_length(&tmp, true);

        bytes.push(0x11); // BufferOp
        bytes.extend_from_slice(&pkg_length);
        bytes.extend_from_slice(&tmp);
        Ok(())
    }
}

impl<'a> ResourceTemplate<'a> {
    pub fn new(children: Vec<&'a dyn Aml>) -> Self {
        ResourceTemplate { children }
    }
}

pub struct Io {
    min: u16,
    max: u16,
    alignment: u8,
    length: u8,
}

impl Io {
    pub fn new(min: u16, max: u16, alignment: u8, length: u8) -> Self {
        Io {
            min,
            max,
            alignment,
            length,
        }
    }
}

impl Aml for Io {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        bytes.push(0x47); // Io Port Descriptor
        bytes.push(1); // IODecode16
        bytes.extend_from_slice(&self.min.to_le_bytes());
        bytes.extend_from_slice(&self.max.to_le_bytes());
        bytes.push(self.alignment);
        bytes.push(self.length);
        Ok(())
    }
}

pub struct Interrupt {
    consumer: bool,
    edge_triggered: bool,
    active_low: bool,
    shared: bool,
    number: u32,
}

impl Interrupt {
    pub fn new(
        consumer: bool,
        edge_triggered: bool,
        active_low: bool,
        shared: bool,
        number: u32,
    ) -> Self {
        Interrupt {
            consumer,
            edge_triggered,
            active_low,
            shared,
            number,
        }
    }
}

impl Aml for Interrupt {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        bytes.push(0x89); // Extended IRQ Descriptor
        bytes.extend_from_slice(&6u16.to_le_bytes());
        let flags = (u8::from(self.shared) << 3)
            | (u8::from(self.active_low) << 2)
            | (u8::from(self.edge_triggered) << 1)
            | u8::from(self.consumer);
        bytes.push(flags);
        bytes.push(1u8); // count
        bytes.extend_from_slice(&self.number.to_le_bytes());
        Ok(())
    }
}

pub struct Device<'a> {
    path: Path,
    children: Vec<&'a dyn Aml>,
}

impl Aml for Device<'_> {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        let mut tmp = Vec::new();
        self.path.append_aml_bytes(&mut tmp)?;

        for child in &self.children {
            child.append_aml_bytes(&mut tmp)?;
        }

        let pkg_length = create_pkg_length(&tmp, true);

        bytes.push(0x5b); // ExtOpPrefix
        bytes.push(0x82); // DeviceOp
        bytes.extend_from_slice(&pkg_length);
        bytes.extend_from_slice(&tmp);
        Ok(())
    }
}

impl<'a> Device<'a> {
    pub fn new(path: Path, children: Vec<&'a dyn Aml>) -> Self {
        Device { path, children }
    }
}

pub struct Method<'a> {
    path: Path,
    children: Vec<&'a dyn Aml>,
    args: u8,
    serialized: bool,
}

impl<'a> Method<'a> {
    pub fn new(path: Path, args: u8, serialized: bool, children: Vec<&'a dyn Aml>) -> Self {
        Method {
            path,
            children,
            args,
            serialized,
        }
    }
}

impl Aml for Method<'_> {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        let mut tmp = Vec::new();
        self.path.append_aml_bytes(&mut tmp)?;
        let flags: u8 = (self.args & 0x7) | (u8::from(self.serialized) << 3);
        tmp.push(flags);
        for child in &self.children {
            child.append_aml_bytes(&mut tmp)?;
        }

        let pkg_length = create_pkg_length(&tmp, true);

        bytes.push(0x14); // MethodOp
        bytes.extend_from_slice(&pkg_length);
        bytes.extend_from_slice(&tmp);
        Ok(())
    }
}

pub struct Return<'a> {
    value: &'a dyn Aml,
}

impl<'a> Return<'a> {
    pub fn new(value: &'a dyn Aml) -> Self {
        Return { value }
    }
}

impl Aml for Return<'_> {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        bytes.push(0xa4); // ReturnOp
        self.value.append_aml_bytes(bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkg_length() {
        assert_eq!(create_pkg_length(&[0u8; 62], true), vec![63]);
        assert_eq!(
            create_pkg_length(&[0u8; 64], true),
            vec![(1 << 6) | (66 & 0xf), 66 >> 4]
        );
        assert_eq!(
            create_pkg_length(&[0u8; 4096], true),
            vec![
                (2 << 6) | (4099 & 0xf) as u8,
                ((4099 >> 4) & 0xff).try_into().unwrap(),
                ((4099 >> 12) & 0xff).try_into().unwrap()
            ]
        );
    }

    #[test]
    fn test_package() {
        // Name (_S5, Package (0x01)  // _S5_: S5 System State
        // {
        // 0x05
        // })
        let s5_sleep_data = [0x08, 0x5F, 0x53, 0x35, 0x5F, 0x12, 0x04, 0x01, 0x0A, 0x05];

        let s5 = Name::new("_S5_".try_into().unwrap(), &Package::new(vec![&5u8])).unwrap();

        assert_eq!(s5_sleep_data.to_vec(), s5.to_aml_bytes().unwrap());
    }

    #[test]
    fn test_eisa_name() {
        assert_eq!(
            Name::new(
                "_HID".try_into().unwrap(),
                &EisaName::new("PNP0501").unwrap()
            )
            .unwrap()
            .to_aml_bytes()
            .unwrap(),
            [0x08, 0x5F, 0x48, 0x49, 0x44, 0x0C, 0x41, 0xD0, 0x05, 0x01],
        )
    }

    #[test]
    fn test_name_path() {
        assert_eq!(
            (&"_SB_".try_into().unwrap() as &Path)
                .to_aml_bytes()
                .unwrap(),
            [0x5Fu8, 0x53, 0x42, 0x5F]
        );
        assert_eq!(
            (&"\\_SB_".try_into().unwrap() as &Path)
                .to_aml_bytes()
                .unwrap(),
            [0x5C, 0x5F, 0x53, 0x42, 0x5F]
        );
        assert_eq!(
            (&"_SB_.COM1".try_into().unwrap() as &Path)
                .to_aml_bytes()
                .unwrap(),
            [0x2E, 0x5F, 0x53, 0x42, 0x5F, 0x43, 0x4F, 0x4D, 0x31]
        );
    }

    #[test]
    fn test_numbers() {
        assert_eq!(128u8.to_aml_bytes().unwrap(), [0x0a, 0x80]);
        assert_eq!(1024u16.to_aml_bytes().unwrap(), [0x0b, 0x0, 0x04]);
        assert_eq!(
            (16u32 << 20).to_aml_bytes().unwrap(),
            [0x0c, 0x00, 0x00, 0x0, 0x01]
        );
        assert_eq!(
            0xdeca_fbad_deca_fbadu64.to_aml_bytes().unwrap(),
            [0x0e, 0xad, 0xfb, 0xca, 0xde, 0xad, 0xfb, 0xca, 0xde]
        );
    }

    #[test]
    fn test_name() {
        assert_eq!(
            Name::new("_SB_.PCI0._UID".try_into().unwrap(), &0x1234u16)
                .unwrap()
                .to_aml_bytes()
                .unwrap(),
            [
                0x08, // NameOp
                0x2F, // MultiNamePrefix
                0x03, // 3 name parts
                0x5F, 0x53, 0x42, 0x5F, // _SB_
                0x50, 0x43, 0x49, 0x30, // PCI0
                0x5F, 0x55, 0x49, 0x44, // _UID
                0x0b, // WordPrefix
                0x34, 0x12
            ]
        );
    }

    #[test]
    fn test_string() {
        assert_eq!(
            (&"ACPI" as &dyn Aml).to_aml_bytes().unwrap(),
            [0x0d, b'A', b'C', b'P', b'I', 0]
        );
        assert_eq!(
            "ACPI".to_owned().to_aml_bytes().unwrap(),
            [0x0d, b'A', b'C', b'P', b'I', 0]
        );
    }

    #[test]
    fn test_method() {
        assert_eq!(
            Method::new(
                "_STA".try_into().unwrap(),
                0,
                false,
                vec![&Return::new(&0xfu8)]
            )
            .to_aml_bytes()
            .unwrap(),
            [0x14, 0x09, 0x5F, 0x53, 0x54, 0x41, 0x00, 0xA4, 0x0A, 0x0F]
        );
    }
}
