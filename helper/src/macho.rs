const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const MAX_FAT_ARCHITECTURES: usize = 64;

pub(crate) fn exact_architecture(bytes: &[u8], architecture: &str) -> Result<Option<bool>, ()> {
    let required = match architecture {
        "x64" => CPU_TYPE_X86_64,
        "arm64" => CPU_TYPE_ARM64,
        _ => return Err(()),
    };
    if bytes.len() < 4 {
        return Ok(None);
    }
    let magic: [u8; 4] = bytes[..4].try_into().map_err(|_| ())?;
    let thin_endian = match magic {
        [0xfe, 0xed, 0xfa, 0xce] | [0xfe, 0xed, 0xfa, 0xcf] => Some(Endian::Big),
        [0xce, 0xfa, 0xed, 0xfe] | [0xcf, 0xfa, 0xed, 0xfe] => Some(Endian::Little),
        _ => None,
    };
    if let Some(endian) = thin_endian {
        return Ok(Some(read_u32(bytes, 4, endian)? == required));
    }
    let (endian, entry_size) = match magic {
        [0xca, 0xfe, 0xba, 0xbe] => (Endian::Big, 20),
        [0xbe, 0xba, 0xfe, 0xca] => (Endian::Little, 20),
        [0xca, 0xfe, 0xba, 0xbf] => (Endian::Big, 32),
        [0xbf, 0xba, 0xfe, 0xca] => (Endian::Little, 32),
        _ => return Ok(None),
    };
    let count = usize::try_from(read_u32(bytes, 4, endian)?).map_err(|_| ())?;
    if count == 0 || count > MAX_FAT_ARCHITECTURES {
        return Err(());
    }
    let required_bytes = 8_usize
        .checked_add(count.checked_mul(entry_size).ok_or(())?)
        .ok_or(())?;
    if bytes.len() < required_bytes {
        return Err(());
    }
    if count != 1 {
        return Ok(Some(false));
    }
    Ok(Some(read_u32(bytes, 8, endian)? == required))
}

#[derive(Clone, Copy)]
enum Endian {
    Big,
    Little,
}

fn read_u32(bytes: &[u8], offset: usize, endian: Endian) -> Result<u32, ()> {
    let value: [u8; 4] = bytes
        .get(offset..offset.checked_add(4).ok_or(())?)
        .ok_or(())?
        .try_into()
        .map_err(|_| ())?;
    Ok(match endian {
        Endian::Big => u32::from_be_bytes(value),
        Endian::Little => u32::from_le_bytes(value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thin(magic: [u8; 4], cpu: u32, endian: Endian) -> Vec<u8> {
        let mut bytes = magic.to_vec();
        bytes.extend_from_slice(&match endian {
            Endian::Big => cpu.to_be_bytes(),
            Endian::Little => cpu.to_le_bytes(),
        });
        bytes
    }

    fn fat(magic: [u8; 4], cpus: &[u32], endian: Endian, entry_size: usize) -> Vec<u8> {
        let mut bytes = magic.to_vec();
        bytes.extend_from_slice(&match endian {
            Endian::Big => (cpus.len() as u32).to_be_bytes(),
            Endian::Little => (cpus.len() as u32).to_le_bytes(),
        });
        for cpu in cpus {
            bytes.extend_from_slice(&match endian {
                Endian::Big => cpu.to_be_bytes(),
                Endian::Little => cpu.to_le_bytes(),
            });
            bytes.resize(bytes.len() + entry_size - 4, 0);
        }
        bytes
    }

    #[test]
    fn thin_architecture_must_match_exactly() {
        let x64 = thin([0xcf, 0xfa, 0xed, 0xfe], CPU_TYPE_X86_64, Endian::Little);
        assert_eq!(exact_architecture(&x64, "x64"), Ok(Some(true)));
        assert_eq!(exact_architecture(&x64, "arm64"), Ok(Some(false)));
        assert_eq!(exact_architecture(b"plain resource", "x64"), Ok(None));
    }

    #[test]
    fn fat32_and_fat64_require_one_exact_slice() {
        let x64_64 = fat(
            [0xca, 0xfe, 0xba, 0xbf],
            &[CPU_TYPE_X86_64],
            Endian::Big,
            32,
        );
        assert_eq!(exact_architecture(&x64_64, "x64"), Ok(Some(true)));

        let universal = fat(
            [0xca, 0xfe, 0xba, 0xbe],
            &[CPU_TYPE_X86_64, CPU_TYPE_ARM64],
            Endian::Big,
            20,
        );
        assert_eq!(exact_architecture(&universal, "x64"), Ok(Some(false)));

        let unlisted = fat(
            [0xbf, 0xba, 0xfe, 0xca],
            &[CPU_TYPE_X86_64, 18],
            Endian::Little,
            32,
        );
        assert_eq!(exact_architecture(&unlisted, "x64"), Ok(Some(false)));
    }

    #[test]
    fn malformed_fat_reports_fail_closed() {
        assert_eq!(
            exact_architecture(&[0xca, 0xfe, 0xba, 0xbf, 0, 0, 0, 1], "arm64"),
            Err(())
        );
    }
}
