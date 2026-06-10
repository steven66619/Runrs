#include "launch.h"
#include <algorithm>
#include <cctype>
#include <unistd.h>
#include <sys/wait.h>

namespace runrs {

bool launch_background(const std::string &command) {
  auto trimmed = command;
  auto notspace = [](unsigned char c) { return !std::isspace(c); };
  auto start = std::find_if(trimmed.begin(), trimmed.end(), notspace);
  auto end = std::find_if(trimmed.rbegin(), trimmed.rend(), notspace).base();
  if (start >= end) return false;
  trimmed = std::string(start, end);

  pid_t pid = fork();
  if (pid == 0) {
    setsid();
    close(STDIN_FILENO);
    close(STDOUT_FILENO);
    close(STDERR_FILENO);
    execl("/bin/sh", "sh", "-c", trimmed.c_str(), nullptr);
    _exit(127);
  }
  return pid > 0;
}

} // namespace runrs
