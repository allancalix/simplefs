#define _FILE_OFFSET_BITS  64
// Required to load up-to-date headers for MacOS.
#define FUSE_USE_VERSION 31

#ifdef __APPLE__
  #include <osxfuse/fuse.h>
#else
  #include <fuse/fuse.h>
#endif
