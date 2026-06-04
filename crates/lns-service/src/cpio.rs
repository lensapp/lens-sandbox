use std::io::{self, Write};

const MAGIC: &[u8] = b"070701";
const TRAILER: &str = "TRAILER!!!";

const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;

#[derive(Debug)]
enum EntryBody {
    Dir,
    File(Vec<u8>),
    Symlink(String),
}

#[derive(Debug)]
struct Entry {
    path: String,
    mode_perms: u32,
    body: EntryBody,
}

#[derive(Default)]
pub struct Initramfs {
    entries: Vec<Entry>,
}

impl Initramfs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_dir(&mut self, path: &str, mode: u32) {
        self.entries.push(Entry {
            path: path.to_string(),
            mode_perms: mode & 0o7777,
            body: EntryBody::Dir,
        });
    }

    pub fn add_file(&mut self, path: &str, data: &[u8], mode: u32) {
        self.entries.push(Entry {
            path: path.to_string(),
            mode_perms: mode & 0o7777,
            body: EntryBody::File(data.to_vec()),
        });
    }

    #[allow(dead_code)]
    pub fn add_symlink(&mut self, path: &str, target: &str) {
        self.entries.push(Entry {
            path: path.to_string(),
            mode_perms: 0o777,
            body: EntryBody::Symlink(target.to_string()),
        });
    }

    pub fn finish<W: Write>(&self, mut w: W) -> io::Result<()> {
        for (ino, e) in (1u32..).zip(self.entries.iter()) {
            let (mode_kind, body, link_target) = match &e.body {
                EntryBody::Dir => (S_IFDIR, &[][..], None),
                EntryBody::File(b) => (S_IFREG, &b[..], None),
                EntryBody::Symlink(t) => (S_IFLNK, &[][..], Some(t.as_bytes())),
            };
            let data: &[u8] = link_target.unwrap_or(body);
            write_entry(&mut w, ino, mode_kind | e.mode_perms, &e.path, data)?;
        }
        write_entry(&mut w, 0, 0, TRAILER, &[])?;
        Ok(())
    }
}

struct NewcHeader {
    ino: u32,
    mode: u32,
    uid: u32,
    gid: u32,
    nlink: u32,
    mtime: u32,
    filesize: u32,
    devmajor: u32,
    devminor: u32,
    rdevmajor: u32,
    rdevminor: u32,
    namesize: u32,
    check: u32,
}

impl NewcHeader {
    fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(MAGIC)?;
        for field in [
            self.ino,
            self.mode,
            self.uid,
            self.gid,
            self.nlink,
            self.mtime,
            self.filesize,
            self.devmajor,
            self.devminor,
            self.rdevmajor,
            self.rdevminor,
            self.namesize,
            self.check,
        ] {
            write_hex(w, field)?;
        }
        Ok(())
    }
}

fn write_entry<W: Write>(
    w: &mut W,
    ino: u32,
    mode: u32,
    name: &str,
    data: &[u8],
) -> io::Result<()> {
    let name_bytes = name.as_bytes();
    let namesize = name_bytes.len() + 1;
    let filesize = data.len();

    NewcHeader {
        ino,
        mode,
        uid: 0,
        gid: 0,
        nlink: 1,
        mtime: 0,
        filesize: filesize as u32,
        devmajor: 0,
        devminor: 0,
        rdevmajor: 0,
        rdevminor: 0,
        namesize: namesize as u32,
        check: 0,
    }
    .write(w)?;

    w.write_all(name_bytes)?;
    w.write_all(&[0])?;

    let header_len = 110 + namesize;
    pad_to_4(w, header_len)?;

    w.write_all(data)?;
    pad_to_4(w, data.len())?;

    Ok(())
}

fn write_hex<W: Write>(w: &mut W, v: u32) -> io::Result<()> {
    let s = format!("{v:08X}");
    debug_assert_eq!(s.len(), 8);
    w.write_all(s.as_bytes())
}

fn pad_to_4<W: Write>(w: &mut W, len: usize) -> io::Result<()> {
    let pad = (4 - (len % 4)) % 4;
    if pad > 0 {
        w.write_all(&[0u8; 3][..pad])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_file_header_layout() {
        let mut fs = Initramfs::new();
        fs.add_file("hi", b"abc", 0o644);
        let mut buf = Vec::new();
        fs.finish(&mut buf).unwrap();

        assert_eq!(&buf[0..6], b"070701", "magic");
        let mode_str = std::str::from_utf8(&buf[14..22]).unwrap();
        let mode = u32::from_str_radix(mode_str, 16).unwrap();
        assert_eq!(mode, S_IFREG | 0o644);

        let fs_str = std::str::from_utf8(&buf[54..62]).unwrap();
        assert_eq!(u32::from_str_radix(fs_str, 16).unwrap(), 3);

        let ns_str = std::str::from_utf8(&buf[94..102]).unwrap();
        assert_eq!(u32::from_str_radix(ns_str, 16).unwrap(), 3);

        assert_eq!(&buf[110..113], b"hi\0");
        assert_eq!(&buf[113..116], &[0, 0, 0]);
        assert_eq!(&buf[116..119], b"abc");
        assert_eq!(buf[119], 0);
        assert_eq!(&buf[120..126], b"070701");
    }

    #[test]
    fn entry_kinds_set_correct_mode_high_bits() {
        let mut fs = Initramfs::new();
        fs.add_dir("d", 0o755);
        fs.add_file("d/f", b"x", 0o600);
        fs.add_symlink("d/l", "f");
        let mut buf = Vec::new();
        fs.finish(&mut buf).unwrap();

        let mut offsets: Vec<usize> = (0..buf.len() - 6)
            .filter(|i| &buf[*i..*i + 6] == b"070701")
            .collect();
        assert_eq!(offsets.len(), 4);
        offsets.truncate(3);

        let modes: Vec<u32> = offsets
            .iter()
            .map(|&o| {
                let s = std::str::from_utf8(&buf[o + 14..o + 22]).unwrap();
                u32::from_str_radix(s, 16).unwrap()
            })
            .collect();
        assert_eq!(modes[0], S_IFDIR | 0o755);
        assert_eq!(modes[1], S_IFREG | 0o600);
        assert_eq!(modes[2], S_IFLNK | 0o777);
    }

    #[test]
    fn trailer_present() {
        let mut fs = Initramfs::new();
        fs.add_file("a", b"", 0o644);
        let mut buf = Vec::new();
        fs.finish(&mut buf).unwrap();
        let last = (0..buf.len() - 6)
            .rfind(|i| &buf[*i..*i + 6] == b"070701")
            .unwrap();
        let name_start = last + 110;
        let name_end = name_start + "TRAILER!!!".len();
        assert_eq!(&buf[name_start..name_end], b"TRAILER!!!");
    }
}
