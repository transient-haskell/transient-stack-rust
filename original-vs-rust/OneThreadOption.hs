{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Control.Monad
import Control.Monad.IO.Class
import Control.Concurrent

-- three "go" events from the console, 100 ms apart; each one launches a 300 ms task
main = keep $ do
    x <- oneThread $ option "go" "go"
    async (threadDelay 300000)
    liftIO $ putStrLn ("  process (option) " ++ show x)
    exit ()
