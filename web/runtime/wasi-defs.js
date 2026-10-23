// WASI preview1 numbers shared by the file-system worker and the thread workers.

export const ERRNO = {
  SUCCESS: 0, ACCES: 2, AGAIN: 6, BADF: 8, EXIST: 20, FAULT: 21, INVAL: 28, IO: 29, ISDIR: 31,
  NAMETOOLONG: 37, NOENT: 44, NOSYS: 52, NOTDIR: 54, NOTEMPTY: 55, NOTSUP: 58, PERM: 63, SPIPE: 70,
};

export const FILETYPE = { UNKNOWN: 0, CHARACTER_DEVICE: 2, DIRECTORY: 3, REGULAR_FILE: 4 };

export const OFLAGS = { CREAT: 1, DIRECTORY: 2, EXCL: 4, TRUNC: 8 };
export const FDFLAGS = { APPEND: 1 };
export const WHENCE = { SET: 0, CUR: 1, END: 2 };

// The syscalls that the file-system worker answers, by number. Each takes up to 10 integer arguments
// (pointers into the calling process's memory, numbers, or 64-bit offsets as BigInts).
export const FS_CALLS = [
  "fd_write", "fd_read", "fd_pread", "fd_pwrite", "fd_seek", "fd_tell", "fd_close", "fd_fdstat_get",
  "fd_fdstat_set_flags", "fd_filestat_get", "fd_filestat_set_size", "fd_filestat_set_times",
  "fd_prestat_get", "fd_prestat_dir_name", "fd_readdir", "fd_sync", "fd_datasync", "fd_advise",
  "fd_allocate", "fd_renumber", "path_open", "path_filestat_get", "path_filestat_set_times",
  "path_create_directory", "path_remove_directory", "path_unlink_file", "path_rename", "path_readlink",
  "path_symlink", "path_link",
];

// The control block a thread shares with the file-system worker, per thread:
// Int32 [0] state, [1] call number / result; BigInt64 arguments from byte 16 (path_open has 9).
export const CTRL_ARGS = 10;
export const CTRL_BYTES = 16 + 8 * CTRL_ARGS;
export const STATE = { IDLE: 0, REQUEST: 1, DONE: 2 };
