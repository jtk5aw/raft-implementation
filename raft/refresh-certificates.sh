#!/bin/sh

# Wipe and recreate the certs/ directory
rm -rf certs/
mkdir certs/

set -xe

openssl ecparam -name secp521r1 -out certs/secp521r1.pem

# CA Cert and key
openssl req -nodes \
          -x509 \
          -days 3650 \
          -newkey ec:certs/secp521r1.pem \
          -keyout certs/ca.key \
          -out certs/ca.cert \
          -sha512 \
          -batch \
          -subj "/CN=ponytown EC CA"

# Intermediate cert request and key
openssl req -nodes \
          -newkey ec:certs/secp521r1.pem \
          -keyout certs/inter.key \
          -out certs/inter.req \
          -sha512 \
          -batch \
          -subj "/CN=ponytown EC level 2 intermediate"

# Risdb and Risdb proxy cert requests and keys
openssl req -nodes \
          -newkey ec:certs/secp521r1.pem \
          -keyout certs/risdb_end.key \
          -out certs/risdb_end.req \
          -sha512 \
          -batch \
          -subj "/CN=risdb-hyper.com"
          
openssl req -nodes \
          -newkey ec:certs/secp521r1.pem \
          -keyout certs/proxy_end.key \
          -out certs/proxy_end.req \
          -sha512 \
          -batch \
          -subj "/CN=risdb-proxy.com"

# Risdb and Risdb proxy private keys
openssl ec \
          -in certs/risdb_end.key \
          -out certs/risdb.ec
          
openssl ec \
          -in certs/proxy_end.key \
          -out certs/proxy.ec

# Risdb and Risdb proxy intermediate cert
openssl x509 -req \
            -in certs/inter.req \
            -out certs/inter.cert \
            -CA certs/ca.cert \
            -CAkey certs/ca.key \
            -sha512 \
            -days 3650 \
            -set_serial 123 \
            -extensions v3_inter -extfile openssl.cnf

# Risdb and Risdb proxy end cert
openssl x509 -req \
            -in certs/risdb_end.req \
            -out certs/risdb_end.cert \
            -CA certs/inter.cert \
            -CAkey certs/inter.key \
            -sha512 \
            -days 2000 \
            -set_serial 456 \
            -extensions v3_end -extfile openssl.cnf
            
openssl x509 -req \
            -in certs/proxy_end.req \
            -out certs/proxy_end.cert \
            -CA certs/inter.cert \
            -CAkey certs/inter.key \
            -sha512 \
            -days 2000 \
            -set_serial 456 \
            -extensions v3_end -extfile openssl.cnf

# Create cert chain TODO: Do some testing to see if this is even nececcsary
cat certs/risdb_end.cert certs/inter.cert certs/ca.cert > certs/risdb.pem
cat certs/proxy_end.cert certs/inter.cert certs/ca.cert > certs/proxy.pem

# Clean up
rm certs/risdb_end.cert certs/proxy_end.cert certs/inter.cert certs/ca.key certs/risdb_end.key certs/proxy_end.key
rm certs/inter.key certs/risdb_end.req certs/proxy_end.req certs/inter.req certs/secp521r1.pem
#rm certs/ca.cert certs/sample.ec certs/sample.pem