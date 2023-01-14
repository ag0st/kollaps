# Base image
FROM alpine:latest

# update repositories
RUN apk update

# Install iperf3
RUN apk add iperf3

# Expose the port for iperf3 tests
EXPOSE 5201

CMD ["iperf3",  "-t", "120", "-c", "192.168.1.102"]
