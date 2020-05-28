// Copyright 2020 Google LLC
//
// Use of this source code is governed by an MIT-style license that can be found
// in the LICENSE file or at https://opensource.org/licenses/MIT.

//! A handler and associated types for the list directory action.
//!
//! A list directory action stats all files in the provided directory.

use crate::session::{self, Session};
use rrg_proto::{ListDirRequest, StatEntry, path_spec::PathType,
                path_spec::Options};

use std::fs::{self, Metadata};
use std::path::{PathBuf, Path};
use std::fmt::{self, Display, Formatter};
use log::warn;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_os = "linux")]
use std::os::raw::c_long;

#[derive(Debug)]
enum Error {
    ReadPath(std::io::Error),
}

impl std::error::Error for Error {

    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        use Error::*;

        match *self {
            ReadPath(ref error) => Some(error),
        }
    }
}

impl Display for Error {

    fn fmt(&self, fmt: &mut Formatter) -> std::fmt::Result {
        use Error::*;

        match *self {
            ReadPath(ref error) => {
                write!(fmt, "unable to read path: {}", error)
            }
        }
    }
}

impl From<Error> for session::Error {

    fn from(error: Error) -> session::Error {
        session::Error::action(error)
    }
}

struct UnsupportedValueMessage {
    field: String,
    value: String,
}

impl std::fmt::Debug for UnsupportedValueMessage {

    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnsupportedValueMessage")
            .field("field", &self.field)
            .field("value", &self.value)
            .finish()
    }
}

#[derive(Debug)]
enum ParseError {
    UnsupportedValue(UnsupportedValueMessage),
}

impl std::error::Error for ParseError {

    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        use ParseError::*;

        match *self {
            UnsupportedValue(_) => None,
        }
    }
}

impl Display for ParseError {

    fn fmt(&self, fmt: &mut Formatter) -> std::fmt::Result {
        use ParseError::*;

        match *self {
            UnsupportedValue(ref message) => {
                write!(fmt, "value {} in {} field is not supported",
                       message.value, message.field)
            }
        }
    }
}

impl From<ParseError> for session::Error {

    fn from(error: ParseError) -> session::Error {
        session::Error::action(error)
    }
}


/// A response type for the list directory action.
pub struct Response {
    mode: Option<u64>,
    ino: Option<u32>,
    dev: Option<u32>,
    nlink: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    size: Option<u64>,
    atime: Option<SystemTime>,
    mtime: Option<SystemTime>,
    ctime: Option<SystemTime>,
    blocks: Option<u32>,
    blksize: Option<u32>,
    rdev: Option<u32>,
    flags_linux: Option<u32>,
    symlink: Option<PathBuf>,
    path: PathBuf,
    crtime: Option<SystemTime>,
}

impl Default for Response {

    fn default() -> Response {
        Response {
            mode: None,
            ino: None,
            dev: None,
            nlink: None,
            uid: None,
            gid: None,
            size: None,
            atime: None,
            mtime: None,
            ctime: None,
            blocks: None,
            blksize: None,
            rdev: None,
            flags_linux: None,
            symlink: None,
            path: Default::default(),
            crtime: None,
        }
    }
}

/// A request type for the list directory action.
pub struct Request {
    path: PathBuf,
}

fn get_accesses_time(metadata: &Metadata) -> Option<SystemTime> {
    match metadata.accessed() {
        Ok(atime) => Some(atime),
        Err(err) => {
            warn!("unable to get last access time: {}", err);
            None
        }
    }
}

fn get_modification_time(metadata: &Metadata) -> Option<SystemTime> {
    match metadata.modified() {
        Ok(atime) => Some(atime),
        Err(err) => {
            warn!("unable to get last modification time: {}", err);
            None
        }
    }
}

fn get_creation_time(metadata: &Metadata) -> Option<SystemTime> {
    match metadata.created() {
        Ok(atime) => Some(atime),
        Err(err) => {
            warn!("unable to get creation time: {}", err);
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn get_status_change_time(metadata: &Metadata) -> Option<SystemTime> {
    use std::time::Duration;
    use std::os::unix::fs::MetadataExt;

    UNIX_EPOCH.checked_add(Duration::from_secs(metadata.ctime() as u64))
}


#[cfg(target_os = "linux")]
fn fill_response(metadata: &Metadata, file_path: &Path) -> Response {
    use std::os::unix::fs::MetadataExt;

    Response {
        mode: Some(metadata.mode().into()),
        ino: Some(metadata.ino() as u32),
        dev: Some(metadata.dev() as u32),
        nlink: Some(metadata.nlink() as u32),
        uid: Some(metadata.uid() as u32),
        gid: Some(metadata.gid() as u32),
        size: Some(metadata.size()),
        atime: get_accesses_time(&metadata),
        mtime: get_modification_time(&metadata),
        ctime: get_status_change_time(&metadata),
        blocks: Some(metadata.blocks() as u32),
        blksize: Some(metadata.blksize() as u32),
        rdev: Some(metadata.rdev() as u32),
        flags_linux:
        Some(get_linux_flags(file_path).unwrap_or_default() as u32),
        symlink: if metadata.file_type().is_symlink() {
            match fs::read_link(file_path) {
                Ok(file) => Some(file),
                Err(error) => {
                    warn!("unable to read symlink: {}", error);
                    None
                }
            }
        } else {
            None
        },
        path: file_path.clone().to_path_buf(),
        crtime: get_creation_time(&metadata),
    }
}

#[cfg(not(target_os = "linux"))]
fn fill_response(metadata: &Metadata, file_path: &Path) -> Response {
    Response {
        size: Some(metadata.len()),
        atime: get_accesses_time(&metadata),
        mtime: get_modification_time(&metadata),
        path: file_path.clone().to_path_buf(),
        crtime: get_creation_time(&metadata),
        ..Default::default()
    }
}

pub fn handle<S: Session>(session: &mut S, request: Request)
                          -> session::Result<()> {
    let dir_path = &request.path;
    let mut paths: Vec<PathBuf> = dir_path.read_dir()
        .map_err(Error::ReadPath)?.filter_map(|entry| entry.ok())
        .map(|entry| entry.path()).collect();
    paths.sort();

    for file_path in &paths {
        let metadata = fs::symlink_metadata(file_path)
            .map_err(Error::ReadPath)?;
        session.reply(fill_response(&metadata, file_path))?;
    }

    Ok(())
}

fn get_path(path: &Option<String>) -> PathBuf {
    match path {
        Some(string_path) if !string_path.is_empty() => {
            PathBuf::from(string_path)
        }
        _ => PathBuf::from("/"),
    }
}

/// Fills st_linux_flags field
#[cfg(target_os = "linux")]
fn get_linux_flags(path: &Path) -> Option<c_long> {
    use std::fs::File;
    use std::os::unix::io::AsRawFd;

    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return None,
    };
    let mut linux_flags: c_long = 0;
    let linux_flags_ptr: *mut c_long = &mut linux_flags;
    unsafe {
        match ioctls::fs_ioc_getflags(file.as_raw_fd(), linux_flags_ptr) {
            0 => Some(linux_flags),
            _ => None,
        }
    }
}

impl super::Request for Request {

    type Proto = ListDirRequest;

    fn from_proto(proto: Self::Proto) -> Result<Request, session::ParseError> {
        let missing = session::MissingFieldError::new;
        let pathspec = proto.pathspec.ok_or(missing("path spec"))?;
        let path_type = pathspec.pathtype
            .ok_or(missing("path type"))?;
        if path_type != PathType::Os as i32 {
            return Err(session::ParseError::malformed
                (ParseError::UnsupportedValue
                    (UnsupportedValueMessage {
                        field: String::from("path type"),
                        value: path_type.to_string(),
                    })));
        }
        let path_option = pathspec.path_options
            .unwrap_or(Options::CaseLiteral as i32);
        if path_option != Options::CaseLiteral as i32 {
            return Err(session::ParseError::malformed
                (ParseError::UnsupportedValue
                    (UnsupportedValueMessage {
                        field: String::from("path option"),
                        value: path_option.to_string(),
                    })));
        };
        Ok(Request {
            path: get_path(&pathspec.path),
        })
    }
}

fn get_time_since_unix_epoch(sys_time: &Option<SystemTime>) -> Option<u64> {
    return sys_time.map_or(None, |time| time.duration_since(UNIX_EPOCH)
        .map_or(None, |dur| Some(dur.as_secs())));
}

impl super::Response for Response {

    const RDF_NAME: Option<&'static str> = Some("StatEntry");

    type Proto = StatEntry;

    fn into_proto(self) -> Self::Proto {
        StatEntry {
            st_mode: self.mode,
            st_ino: self.ino,
            st_dev: self.dev,
            st_nlink: self.nlink,
            st_uid: self.uid,
            st_gid: self.gid,
            st_size: self.size,
            st_atime: get_time_since_unix_epoch(&self.atime),
            st_mtime: get_time_since_unix_epoch(&self.mtime),
            st_ctime: get_time_since_unix_epoch(&self.ctime),
            st_blocks: self.blocks,
            st_blksize: self.blksize,
            st_rdev: self.rdev,
            st_flags_osx: None,
            st_flags_linux: self.flags_linux,
            symlink:
            self.symlink
                .map_or(None,
                        |symlink| Some(symlink.to_string_lossy().to_string())),
            registry_type: None,
            resident: None,
            pathspec: Some(rrg_proto::PathSpec {
                // represents CaseLiteral path option (other options are not
                // supported)
                path_options: Some(Options::CaseLiteral as i32),
                // represents OS path type (other types are not supported)
                pathtype: Some(PathType::Os as i32),
                path: Some(self.path.to_string_lossy().to_string()),
                ..Default::default()
            }),
            registry_data: None,
            st_crtime: get_time_since_unix_epoch(&self.crtime),
            ext_attrs: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Request;
    use tempfile::tempdir;

    #[cfg(target_os = "linux")]
    use std::os::unix::fs::MetadataExt;

    /// Fills ListDirRequest with provided fields
    fn fill_proto_request(path_options: Option<i32>,
                          pathtype: Option<i32>,
                          path: Option<String>) -> ListDirRequest {
        ListDirRequest {
            pathspec: Some(rrg_proto::PathSpec {
                path_options,
                pathtype,
                path,
                ..Default::default()
            }),
            iterator: None,
        }
    }

    #[test]
    fn test_empty_pathspec_field() {
        let request: Result<super::Request, _> =
            Request::from_proto(ListDirRequest {
                pathspec: None,
                iterator: None,
            });
        assert!(request.is_err());
    }

    #[test]
    fn test_empty_path_options() {
        let request: Result<super::Request, _> = Request::from_proto
            (fill_proto_request(None, Some(PathType::Os as i32),
                                Some(String::from("/"))));
        assert!(request.is_ok());
    }

    #[test]
    fn test_unsupported_path_options() {
        let request: Result<super::Request, _> = Request::from_proto
            (fill_proto_request(Some(Options::Regex as i32),
                                Some(PathType::Os as i32),
                                Some(String::from("/"))));
        assert!(request.is_err());
    }

    #[test]
    fn test_ok_path_options() {
        let request: Result<super::Request, _> = Request::from_proto
            (fill_proto_request(Some(Options::CaseLiteral as i32),
                                Some(PathType::Os as i32),
                                Some(String::from("/"))));
        assert!(request.is_ok());
    }

    #[test]
    fn test_unset_pathtype() {
        let request: Result<super::Request, _> = Request::from_proto
            (fill_proto_request(None, Some(PathType::Unset as i32),
                                Some(String::from("/"))));
        assert!(request.is_err());
    }

    #[test]
    fn test_unsupported_pathtype() {
        let request: Result<super::Request, _> = Request::from_proto
            (fill_proto_request(None, Some(PathType::Tsk as i32),
                                Some(String::from("/"))));
        assert!(request.is_err());
    }


    #[test]
    fn test_empty_pathtype() {
        let request: Result<super::Request, _> = Request::from_proto
            (fill_proto_request(None, None, Some(String::from("/"))));
        assert!(request.is_err());
    }

    #[test]
    fn test_empty_path() {
        let request: Result<super::Request, _> = Request::from_proto
            (fill_proto_request(None, Some(PathType::Os as i32), None));
        assert!(&request.is_ok());
        assert_eq!(request.unwrap().path, PathBuf::from("/"));
    }

    #[test]
    fn test_empty_dir() {
        let dir = tempdir().unwrap();
        let request = super::Request {
            path: PathBuf::from(dir.path()),
        };
        let mut session = session::test::Fake::new();
        assert!(handle(&mut session, request).is_ok());
        assert_eq!(session.reply_count(), 0);
    }

    #[test]
    fn test_nonexistent_path() {
        let dir = tempdir().unwrap();
        let request = super::Request {
            path: PathBuf::from(dir.path().join("nonexistent_subdir")),
        };
        let mut session = session::test::Fake::new();
        assert!(handle(&mut session, request).is_err());
    }

    #[test]
    fn test_lexicographical_order() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        std::fs::File::create(dir_path.join("юникод")).unwrap();
        std::fs::File::create(dir_path.join("unicode")).unwrap();
        std::fs::File::create(dir_path.join("file")).unwrap();
        std::fs::File::create(dir_path.join("afile")).unwrap();
        std::fs::File::create(dir_path.join("Datei")).unwrap();
        std::fs::File::create(dir_path.join("snake_case")).unwrap();
        std::fs::File::create(dir_path.join("CamelCase")).unwrap();
        let request = super::Request {
            path: PathBuf::from(&dir_path),
        };
        let mut session = session::test::Fake::new();
        assert!(handle(&mut session, request).is_ok());
        assert_eq!(session.reply_count(), 7);
        assert_eq!(&session.reply::<Response>(0).path,
                   &dir_path.join("CamelCase"));
        assert_eq!(&session.reply::<Response>(1).path,
                   &dir_path.join("Datei"));
        assert_eq!(&session.reply::<Response>(2).path,
                   &dir_path.join("afile"));
        assert_eq!(&session.reply::<Response>(3).path,
                   &dir_path.join("file"));
        assert_eq!(&session.reply::<Response>(4).path,
                   &dir_path.join("snake_case"));
        assert_eq!(&session.reply::<Response>(5).path,
                   &dir_path.join("unicode"));
        assert_eq!(&session.reply::<Response>(6).path,
                   &dir_path.join("юникод"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_dir_response() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let inner_dir_path = &dir_path.join("dir");
        std::fs::create_dir(&inner_dir_path).unwrap();
        let request = super::Request {
            path: PathBuf::from(&dir_path),
        };
        let mut session = session::test::Fake::new();
        handle(&mut session, request).unwrap();
        assert_eq!(session.reply_count(), 1);
        let inner_dir = &session.reply::<Response>(0);
        assert_eq!(&inner_dir.path, inner_dir_path);
        assert!(inner_dir.symlink.is_none());
        assert_eq!(inner_dir.uid.unwrap(), users::get_current_uid());
        assert_eq!(inner_dir.gid.unwrap(), users::get_current_uid());
        assert_eq!(inner_dir.dev.unwrap(),
                   dir_path.metadata().unwrap().dev() as u32);
        assert_eq!(inner_dir.mode.unwrap(), 0o40775);
        assert_eq!(inner_dir.nlink.unwrap(), 2);
        assert!(inner_dir.atime.unwrap() <= SystemTime::now());
        assert!(inner_dir.ctime.unwrap() <= SystemTime::now());
        assert!(inner_dir.mtime.unwrap() <= SystemTime::now());
        assert!(inner_dir.crtime.unwrap() <= SystemTime::now());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_symlink_response() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let file_path = dir_path.join("file");
        std::fs::File::create(&file_path).unwrap();
        let sl_path = dir_path.join("symlink");
        std::os::unix::fs::symlink(&file_path, &sl_path).unwrap();
        let request = super::Request {
            path: PathBuf::from(&dir_path),
        };
        let mut session = session::test::Fake::new();
        assert!(handle(&mut session, request).is_ok());
        assert_eq!(session.reply_count(), 2);
        let symlink = &session.reply::<Response>(1);
        assert_eq!(&symlink.path, &sl_path);
        assert!(&symlink.symlink.is_some());
        assert_eq!(&symlink.symlink, &Some(file_path));
        assert_eq!(symlink.mode.unwrap(), 0o120777);
        assert_eq!(symlink.nlink.unwrap(), 1);
        assert!(symlink.atime.unwrap() <= SystemTime::now());
        assert!(symlink.ctime.unwrap() <= SystemTime::now());
        assert!(symlink.mtime.unwrap() <= SystemTime::now());
        assert!(symlink.crtime.unwrap() <= SystemTime::now());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_file_response_linux() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let file_path = dir_path.join("file");
        std::fs::File::create(&file_path).unwrap();
        std::fs::set_permissions(&file_path,
                                 PermissionsExt::from_mode(0o664)).unwrap();
        let request = super::Request {
            path: PathBuf::from(&dir_path),
        };
        let mut session = session::test::Fake::new();
        assert!(handle(&mut session, request).is_ok());
        assert_eq!(session.reply_count(), 1);
        let file = &session.reply::<Response>(0);
        assert_eq!(file.path, file_path);
        assert_eq!(file.size.unwrap(), 0);
        assert_eq!(file.mode.unwrap(), 0o100664);
        assert_eq!(file.uid.unwrap(), users::get_current_uid());
        assert_eq!(file.gid.unwrap(), users::get_current_uid());
        assert_eq!(file.dev.unwrap(),
                   dir_path.metadata().unwrap().dev() as u32);
        assert_eq!(file.nlink.unwrap(), 1);
        assert!(file.symlink.is_none());
        assert!(file.atime.unwrap() <= SystemTime::now());
        assert!(file.ctime.unwrap() <= SystemTime::now());
        assert!(file.mtime.unwrap() <= SystemTime::now());
        assert!(file.crtime.unwrap() <= SystemTime::now());
    }

    #[test]
    fn test_file_response() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let file_path = dir_path.join("file");
        std::fs::File::create(&file_path).unwrap();
        let request = super::Request {
            path: PathBuf::from(&dir_path),
        };
        let mut session = session::test::Fake::new();
        assert!(handle(&mut session, request).is_ok());
        assert_eq!(session.reply_count(), 1);
        let file = &session.reply::<Response>(0);
        assert_eq!(file.path, file_path);
        assert_eq!(file.size.unwrap(), 0);
        assert!(file.atime.unwrap() <= SystemTime::now());
        assert!(file.mtime.unwrap() <= SystemTime::now());
        assert!(file.crtime.unwrap() <= SystemTime::now());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_st_flags_linux() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let file_path = dir_path.join("file");
        std::fs::File::create(&file_path).unwrap();
        let request = super::Request {
            path: PathBuf::from(&dir_path),
        };
        let mut session = session::test::Fake::new();
        assert!(handle(&mut session, request).is_ok());
        assert_eq!(session.reply_count(), 1);
        let file = &session.reply::<Response>(0);
        assert_eq!(file.path, file_path);
        assert_ne!(file.flags_linux.unwrap(), 0);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_linux_flags_non_existing_path() {
        let path_buf = PathBuf::from("some non existing path");
        assert!(get_linux_flags(&path_buf).is_none());
    }

    #[test]
    fn test_unicode_paths() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        std::fs::File::create(dir_path.join("❤ℝℝG❤")).unwrap();
        std::fs::File::create(dir_path.join("файл")).unwrap();
        std::fs::File::create(dir_path.join("ファイル")).unwrap();
        std::fs::File::create(dir_path.join("αρχείο")).unwrap();
        std::fs::File::create(dir_path.join("फ़ाइल")).unwrap();
        let request = super::Request {
            path: PathBuf::from(&dir_path),
        };
        let mut session = session::test::Fake::new();
        assert!(handle(&mut session, request).is_ok());
        assert_eq!(session.reply_count(), 5);
        let file = &session.reply::<Response>(0);
        assert_eq!(file.size.unwrap(), 0);
        assert!(file.atime.unwrap() <= SystemTime::now());
        assert!(file.mtime.unwrap() <= SystemTime::now());
        assert!(file.crtime.unwrap() <= SystemTime::now());
        let file = &session.reply::<Response>(1);
        assert_eq!(file.size.unwrap(), 0);
        assert!(file.atime.unwrap() <= SystemTime::now());
        assert!(file.mtime.unwrap() <= SystemTime::now());
        assert!(file.crtime.unwrap() <= SystemTime::now());
        let file = &session.reply::<Response>(2);
        assert_eq!(file.size.unwrap(), 0);
        assert!(file.atime.unwrap() <= SystemTime::now());
        assert!(file.mtime.unwrap() <= SystemTime::now());
        assert!(file.crtime.unwrap() <= SystemTime::now());
        let file = &session.reply::<Response>(3);
        assert_eq!(file.size.unwrap(), 0);
        assert!(file.atime.unwrap() <= SystemTime::now());
        assert!(file.mtime.unwrap() <= SystemTime::now());
        assert!(file.crtime.unwrap() <= SystemTime::now());
        let file = &session.reply::<Response>(4);
        assert_eq!(file.size.unwrap(), 0);
        assert!(file.atime.unwrap() <= SystemTime::now());
        assert!(file.mtime.unwrap() <= SystemTime::now());
        assert!(file.crtime.unwrap() <= SystemTime::now());
    }
}
