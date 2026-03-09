# Project:   hyperi-rustlib
# File:      Makefile
# Purpose:   CI targets wrapping hyperi-ci
#
# License:   FSL-1.1-ALv2
# Copyright: (c) 2026 HYPERI PTY LIMITED

.PHONY: quality test build

quality:
	hyperi-ci run quality

test:
	hyperi-ci run test

build:
	hyperi-ci run build
