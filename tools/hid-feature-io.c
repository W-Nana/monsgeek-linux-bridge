#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/hidraw.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#define REPORT_BYTES 64

static int hex_value(char digit) {
  if (digit >= '0' && digit <= '9') {
    return digit - '0';
  }
  digit = (char)tolower((unsigned char)digit);
  if (digit >= 'a' && digit <= 'f') {
    return digit - 'a' + 10;
  }
  return -1;
}

static int parse_hex_report(const char *hex, unsigned char *report) {
  size_t count = 0;
  int high = -1;
  for (const char *cursor = hex; *cursor; cursor++) {
    if (isspace((unsigned char)*cursor) || *cursor == ':' || *cursor == '-') {
      continue;
    }
    int value = hex_value(*cursor);
    if (value < 0) {
      fprintf(stderr, "invalid hex digit: %c\n", *cursor);
      return -1;
    }
    if (high < 0) {
      high = value;
      continue;
    }
    if (count >= REPORT_BYTES) {
      fprintf(stderr, "payload is longer than %d bytes\n", REPORT_BYTES);
      return -1;
    }
    report[count++] = (unsigned char)((high << 4) | value);
    high = -1;
  }
  if (high >= 0) {
    fprintf(stderr, "hex payload has an odd number of digits\n");
    return -1;
  }
  return 0;
}

static void print_hex(const unsigned char *bytes, size_t count) {
  for (size_t index = 0; index < count; index++) {
    printf("%02x", bytes[index]);
  }
  printf("\n");
}

static int open_hidraw(const char *device) {
  int fd = open(device, O_RDWR | O_NONBLOCK);
  if (fd < 0) {
    fprintf(stderr, "open %s: %s\n", device, strerror(errno));
  }
  return fd;
}

static int send_report(const char *device, const char *hex) {
  unsigned char ioctl_report[REPORT_BYTES + 1] = {0};
  if (parse_hex_report(hex, ioctl_report + 1) < 0) {
    return 2;
  }

  int fd = open_hidraw(device);
  if (fd < 0) {
    return 1;
  }
  int result = ioctl(fd, HIDIOCSFEATURE(sizeof(ioctl_report)), ioctl_report);
  if (result < 0) {
    fprintf(stderr, "HIDIOCSFEATURE %s: %s\n", device, strerror(errno));
    close(fd);
    return 1;
  }
  close(fd);
  printf("%d\n", result);
  return 0;
}

static int read_report(const char *device) {
  unsigned char ioctl_report[REPORT_BYTES + 1] = {0};
  int fd = open_hidraw(device);
  if (fd < 0) {
    return 1;
  }
  int result = ioctl(fd, HIDIOCGFEATURE(sizeof(ioctl_report)), ioctl_report);
  if (result < 0) {
    fprintf(stderr, "HIDIOCGFEATURE %s: %s\n", device, strerror(errno));
    close(fd);
    return 1;
  }
  close(fd);
  print_hex(ioctl_report + 1, REPORT_BYTES);
  return 0;
}

int main(int argc, char **argv) {
  if (argc == 4 && strcmp(argv[1], "send") == 0) {
    return send_report(argv[2], argv[3]);
  }
  if (argc == 3 && strcmp(argv[1], "read") == 0) {
    return read_report(argv[2]);
  }
  fprintf(stderr, "usage: %s send /dev/hidrawN <hex-64-byte-payload>\n", argv[0]);
  fprintf(stderr, "       %s read /dev/hidrawN\n", argv[0]);
  return 2;
}
