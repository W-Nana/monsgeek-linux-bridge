#include <errno.h>
#include <fcntl.h>
#include <linux/hidraw.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#define REPORT_BYTES 64
#define GET_REV 0x80

static void print_bytes(const unsigned char *bytes, size_t count) {
  for (size_t index = 0; index < count; index++) {
    printf("%02x%s", bytes[index], index + 1 == count ? "\n" : " ");
  }
}

int main(int argc, char **argv) {
  const char *device = argc > 1 ? argv[1] : "/dev/hidraw4";
  int fd = open(device, O_RDWR | O_NONBLOCK);
  if (fd < 0) {
    fprintf(stderr, "open %s: %s\n", device, strerror(errno));
    return 1;
  }

  char name[256] = {0};
  if (ioctl(fd, HIDIOCGRAWNAME(sizeof(name)), name) >= 0) {
    printf("device: %s\n", name);
  }

  /*
   * hidraw feature ioctls include a report-id byte. This interface has no
   * report id in its descriptor, so byte 0 stays zero and the 64-byte
   * MonsGeek payload begins at byte 1.
   */
  unsigned char write_report[REPORT_BYTES + 1] = {0};
  write_report[1] = GET_REV;
  write_report[8] = 0xff - GET_REV;
  printf("query payload: ");
  print_bytes(write_report + 1, REPORT_BYTES);

  int written = ioctl(fd, HIDIOCSFEATURE(sizeof(write_report)), write_report);
  if (written < 0) {
    fprintf(stderr, "HIDIOCSFEATURE GET_REV: %s\n", strerror(errno));
    close(fd);
    return 1;
  }
  printf("feature bytes written: %d\n", written);

  unsigned char read_report[REPORT_BYTES + 1] = {0};
  int read = ioctl(fd, HIDIOCGFEATURE(sizeof(read_report)), read_report);
  if (read < 0) {
    fprintf(stderr, "HIDIOCGFEATURE GET_REV: %s\n", strerror(errno));
    close(fd);
    return 1;
  }

  printf("feature bytes read: %d\n", read);
  printf("reply report id: %02x\n", read_report[0]);
  printf("reply payload: ");
  print_bytes(read_report + 1, REPORT_BYTES);
  if (read >= 4 && read_report[1] == GET_REV) {
    printf("firmware revision word: 0x%02x%02x\n", read_report[3], read_report[2]);
  }

  close(fd);
  return 0;
}
