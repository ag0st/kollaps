# Base image
FROM alpine:latest

# update repositories
RUN apk update

# Install iperf3
RUN apk add iperf3

# Expose the port for iperf3 tests
EXPOSE 5201